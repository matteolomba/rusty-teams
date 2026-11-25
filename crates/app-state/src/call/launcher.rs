use std::future::Future;
use std::sync::{Arc, Mutex};

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

use calling::meeting::{LiveMeeting, fetch_live_meeting};
use calling::{
    AcceptMode, BackgroundCache, CallControl, CallEngine, CallHandle, CallSpec, CallState,
    CallUpdate, DefaultBackground, DeviceChoice, EndReason, EngineConfig, EngineEvent,
    MeetingTarget, MicSettings, call_channel,
};
use session::{Session, SessionConfig, Transport};

use super::schedule::{MeetingDraft, attendee_for};

const START_RETRY_STEPS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
const START_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(20);
const START_JITTER: Duration = Duration::from_millis(500);
const START_CANCELLED: &str = "engine start cancelled";
pub(super) const NOT_READY: &str =
    "Could not reach the Teams calling service. Check your connection and try again.";
const READY_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LaunchState {
    Starting,
    Ready,
    Failed(String),
}

pub struct CallLauncher {
    engine: watch::Sender<Option<CallEngine>>,
    state: watch::Sender<LaunchState>,
    cancel: watch::Sender<bool>,
    default_camera: Mutex<DeviceChoice>,
    default_mic: Mutex<MicSettings>,
}

impl CallLauncher {
    pub fn start(
        transport: Arc<dyn Transport>,
        ringable: bool,
        on_event: impl Fn(EngineEvent) + Send + 'static,
    ) -> Arc<CallLauncher> {
        Self::start_with(
            transport,
            ringable,
            on_event,
            &START_RETRY_STEPS,
            START_ATTEMPT_TIMEOUT,
            START_JITTER,
        )
    }

    fn start_with(
        transport: Arc<dyn Transport>,
        ringable: bool,
        on_event: impl Fn(EngineEvent) + Send + 'static,
        steps: &'static [Duration],
        attempt_timeout: Duration,
        jitter: Duration,
    ) -> Arc<CallLauncher> {
        let launcher = Arc::new(CallLauncher {
            engine: watch::channel(None).0,
            state: watch::channel(LaunchState::Starting).0,
            cancel: watch::channel(false).0,
            default_camera: Mutex::new(DeviceChoice::SystemDefault),
            default_mic: Mutex::new(MicSettings::default()),
        });
        let running = launcher.clone();
        let cancelled = launcher.cancel.subscribe();
        crate::runtime::handle().spawn(async move {
            let outcome = start_with_retry(
                || start_engine(transport.clone(), ringable),
                steps,
                attempt_timeout,
                jitter,
                cancelled,
            )
            .await;
            match outcome {
                Ok((engine, mut events)) => {
                    crate::diagnostics::record(
                        "calls: engine ready, registered with the registrar",
                    );
                    engine.set_default_camera(
                        running
                            .default_camera
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone(),
                    );
                    engine.set_default_mic(
                        *running
                            .default_mic
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()),
                    );
                    running.state.send_replace(LaunchState::Ready);
                    running.engine.send_replace(Some(engine));
                    while let Some(event) = events.recv().await {
                        on_event(event);
                    }
                }
                Err(failure) => {
                    crate::diagnostics::record(format!("calls: engine start stopped: {failure}"));
                    running.state.send_replace(LaunchState::Failed(failure));
                    running.engine.send_replace(None);
                }
            }
        });
        launcher
    }

    pub(super) fn launch_state(&self) -> LaunchState {
        self.state.borrow().clone()
    }

    #[cfg(test)]
    pub(super) fn for_test() -> Arc<Self> {
        Arc::new(CallLauncher {
            engine: watch::channel(None).0,
            state: watch::channel(LaunchState::Starting).0,
            cancel: watch::channel(false).0,
            default_camera: Mutex::new(DeviceChoice::SystemDefault),
            default_mic: Mutex::new(MicSettings::default()),
        })
    }

    fn engine(&self) -> Option<CallEngine> {
        self.engine.borrow().clone()
    }

    async fn ready_engine(&self) -> Option<CallEngine> {
        wait_until_ready(self.engine.subscribe(), READY_WAIT).await
    }

    fn start_when_ready(&self, spec: CallSpec, kind: OutgoingKind) -> CallHandle {
        let (handle, control) = call_channel();
        let ready = self.engine.subscribe();
        crate::runtime::handle().spawn(queue_outgoing(ready, READY_WAIT, spec, kind, control));
        handle
    }

    pub fn set_default_camera(&self, camera: DeviceChoice) {
        *self
            .default_camera
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = camera.clone();
        if let Some(engine) = self.engine() {
            engine.set_default_camera(camera);
        }
    }

    pub fn set_default_mic(&self, mic: MicSettings) {
        *self
            .default_mic
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = mic;
        if let Some(engine) = self.engine() {
            engine.set_default_mic(mic);
        }
    }

    pub fn start_call(&self, spec: CallSpec) -> CallHandle {
        match self.engine() {
            Some(engine) => engine.start_call(spec),
            None => self.start_when_ready(spec, OutgoingKind::Audio),
        }
    }

    pub fn start_video_call(&self, spec: CallSpec) -> CallHandle {
        match self.engine() {
            Some(engine) => engine.start_video_call(spec),
            None => self.start_when_ready(spec, OutgoingKind::Video),
        }
    }

    pub fn start_consult(&self, spec: CallSpec) -> CallHandle {
        match self.engine() {
            Some(engine) => engine.start_consult(spec),
            None => self.start_when_ready(spec, OutgoingKind::Consult),
        }
    }

    pub async fn accept_ring(&self, ring_id: u64, mode: AcceptMode) -> Option<CallHandle> {
        self.engine()?.accept_ring(ring_id, mode).await
    }

    pub fn decline_ring(&self, ring_id: u64) {
        if let Some(engine) = self.engine() {
            engine.decline_ring(ring_id);
        }
    }

    pub fn drop_ring(&self, ring_id: u64) {
        if let Some(engine) = self.engine() {
            engine.drop_ring(ring_id);
        }
    }

    pub async fn refresh_backgrounds(
        &self,
        cache: &BackgroundCache,
        thumbnails: usize,
    ) -> Option<Vec<DefaultBackground>> {
        let session = self.engine()?.session();
        let catalog = cache.refresh_catalog(&session).await.ok()?;
        for image in catalog.iter().take(thumbnails) {
            let _ = cache.ensure_thumbnail(&session, image).await;
        }
        Some(catalog)
    }

    pub async fn download_background(
        &self,
        cache: &BackgroundCache,
        image: &DefaultBackground,
    ) -> Option<std::path::PathBuf> {
        let session = self.engine()?.session();
        cache.ensure_image(&session, image).await.ok()
    }

    pub async fn resolve_meeting(
        &self,
        meeting_data: &serde_json::Value,
    ) -> Result<MeetingTarget, String> {
        self.ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .resolve_meeting(meeting_data)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn create_meet_now(&self, subject: &str) -> Result<String, String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .create_meet_now(subject)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn resolve_new_event(
        &self,
        draft: MeetingDraft,
    ) -> Result<chatsvc::NewEvent, String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        let graph = graph::Graph::new(session);
        let mut attendees = Vec::with_capacity(draft.people.len());
        for invitee in &draft.people {
            let looked_up = match invitee.mail {
                Some(_) => None,
                None => graph.user_mail(&invitee.user_id).await.ok().flatten(),
            };
            attendees.push(attendee_for(invitee, looked_up)?);
        }
        Ok(chatsvc::NewEvent {
            subject: draft.subject,
            start: draft.start,
            end: draft.end,
            local_start: draft.local_start,
            local_end: draft.local_end,
            time_zone_name: draft.time_zone_name,
            attendees,
            repeat: draft.repeat,
            details: draft.details,
        })
    }

    pub async fn create_meeting(
        &self,
        draft: MeetingDraft,
    ) -> Result<chatsvc::CreatedEvent, String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        let event = self.resolve_new_event(draft).await?;
        chatsvc::Calendar::new(&session)
            .create_event(&event)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn create_meeting_with_join(
        &self,
        event: chatsvc::NewEvent,
        join_url: &str,
    ) -> Result<chatsvc::CreatedEvent, String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .create_event_with_join(&event, join_url)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn time_zone_name(&self, mail: &str) -> Option<String> {
        let session = self.engine()?.session();
        graph::Graph::new(session)
            .time_zone_name(mail)
            .await
            .ok()
            .flatten()
    }

    pub async fn respond_to_event(
        &self,
        event_id: &str,
        ical_uid: Option<&str>,
        reply: chatsvc::Reply,
        send_response: bool,
        comment: &str,
    ) -> Result<(), String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .respond(event_id, ical_uid, reply, send_response, comment)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn calendar_event_details(
        &self,
        event_id: &str,
        ical_uid: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .event_details(event_id, ical_uid)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn delete_calendar_event(
        &self,
        event_id: &str,
        ical_uid: Option<&str>,
    ) -> Result<(), String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .delete_event(event_id, ical_uid)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn calendar_view(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<chatsvc::CalendarEvent>, String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .calendar_view(start, end)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn reminder_view(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<chatsvc::Reminder>, String> {
        let session = self.engine().ok_or_else(|| NOT_READY.to_owned())?.session();
        chatsvc::Calendar::new(&session)
            .reminder_view(start, end)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn snooze_reminder(
        &self,
        event_id: &str,
        until: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .snooze_reminder(event_id, until)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn dismiss_reminder(&self, event_id: &str) -> Result<(), String> {
        let session = self
            .ready_engine()
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .session();
        chatsvc::Calendar::new(&session)
            .dismiss_reminder(event_id)
            .await
            .map_err(|error| error.to_string())
    }
    pub async fn live_meeting(&self, thread_id: &str) -> Result<Option<LiveMeeting>, String> {
        let session = self.engine().ok_or_else(|| NOT_READY.to_owned())?.session();
        fetch_live_meeting(&session, thread_id)
            .await
            .map_err(|error| error.to_string())
    }
}

impl Drop for CallLauncher {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
    }
}

async fn start_with_retry<T, E, F, S>(
    mut start: S,
    steps: &[Duration],
    attempt_timeout: Duration,
    jitter: Duration,
    mut cancel: watch::Receiver<bool>,
) -> Result<T, String>
where
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
    F: Future<Output = Result<T, E>> + Send + 'static,
    S: FnMut() -> F,
{
    let mut last_failure = String::new();
    for index in 0..=steps.len() {
        if *cancel.borrow() {
            return Err(START_CANCELLED.to_owned());
        }
        if index > 0 {
            let delay = jittered_delay(steps[index - 1], jitter);
            crate::diagnostics::record(format!(
                "calls: engine start failed, retrying in {}ms: {last_failure}",
                delay.as_millis()
            ));
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = cancel.changed() => return Err(START_CANCELLED.to_owned()),
            }
        }
        let mut task = tokio::spawn(start());
        let abort = task.abort_handle();
        let failure = tokio::select! {
            result = &mut task => match result {
                Ok(Ok(started)) => return Ok(started),
                Ok(Err(error)) => error.to_string(),
                Err(join_error) => format!("start task panicked: {join_error}"),
            },
            _ = tokio::time::sleep(attempt_timeout) => {
                abort.abort();
                format!("start timed out after {}ms", attempt_timeout.as_millis())
            }
            _ = cancel.changed() => {
                abort.abort();
                return Err(START_CANCELLED.to_owned());
            }
        };
        if failure != last_failure {
            last_failure = failure;
        }
    }
    Err(last_failure)
}

fn jittered_delay(base: Duration, jitter: Duration) -> Duration {
    if jitter.is_zero() {
        return base;
    }
    let span = jitter.as_nanos().min(u64::MAX as u128) as u64;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    base + Duration::from_nanos(nanos % (span + 1))
}

async fn start_engine(
    transport: Arc<dyn Transport>,
    ringable: bool,
) -> calling::Result<(
    CallEngine,
    tokio::sync::mpsc::UnboundedReceiver<EngineEvent>,
)> {
    let session = Session::with_transport(transport.clone(), SessionConfig::for_app()).await?;
    let poll_session = Session::with_transport(transport, SessionConfig::for_app()).await?;
    let dump_dir = crate::support::call_dumps_dir();
    let config = EngineConfig {
        ringable,
        dump_dir,
        ..EngineConfig::default()
    };
    CallEngine::start(session, poll_session, config).await
}

#[derive(Clone, Copy)]
enum OutgoingKind {
    Audio,
    Video,
    Consult,
}

async fn wait_until_ready<T: Clone>(
    mut receiver: watch::Receiver<Option<T>>,
    timeout: Duration,
) -> Option<T> {
    let ready = tokio::time::timeout(timeout, receiver.wait_for(Option::is_some))
        .await
        .ok()?
        .ok()?;
    ready.clone()
}

async fn queue_outgoing(
    ready: watch::Receiver<Option<CallEngine>>,
    timeout: Duration,
    spec: CallSpec,
    kind: OutgoingKind,
    mut control: CallControl,
) {
    let ended = |reason| CallUpdate::State(CallState::Ended { reason });
    match control.until_hangup(wait_until_ready(ready, timeout)).await {
        Err(_) => control.send(ended(EndReason::Cancelled)),
        Ok(None) => control.send(ended(EndReason::Failed(NOT_READY.to_owned()))),
        Ok(Some(engine)) => {
            let (concurrent, start_camera) = match kind {
                OutgoingKind::Audio => (false, false),
                OutgoingKind::Video => (false, true),
                OutgoingKind::Consult => (true, false),
            };
            engine.start_call_on(spec, concurrent, start_camera, control);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use calling::CallCommand;

    use super::*;

    fn test_launcher() -> CallLauncher {
        CallLauncher {
            engine: watch::channel(None).0,
            state: watch::channel(LaunchState::Starting).0,
            cancel: watch::channel(false).0,
            default_camera: Mutex::new(DeviceChoice::SystemDefault),
            default_mic: Mutex::new(MicSettings::default()),
        }
    }

    #[test]
    fn a_poisoned_camera_lock_does_not_break_later_camera_changes() {
        let launcher = test_launcher();
        let camera = &launcher.default_camera;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = camera.lock().unwrap();
            panic!("Poisoning the lock");
        }));
        launcher.set_default_camera(DeviceChoice::Device("camera-1".into()));
        assert!(matches!(
            &*launcher
                .default_camera
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            DeviceChoice::Device(id) if id == "camera-1"
        ));
    }

    #[test]
    fn engine_start_survives_errors_and_panics() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let (_sender, cancel) = watch::channel(false);
        let started = runtime.block_on(start_with_retry(
            move || {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    match attempt {
                        0 => panic!("boom"),
                        1 => Err("registrar_http_500".to_owned()),
                        _ => Ok(attempt),
                    }
                }
            },
            &[Duration::ZERO, Duration::ZERO],
            Duration::from_secs(1),
            Duration::ZERO,
            cancel,
        ));
        assert_eq!(started, Ok(2));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn engine_start_gives_up_after_the_last_attempt() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let (_sender, cancel) = watch::channel(false);
        let outcome = start_with_retry(
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { Err::<u32, _>("registrar_http_500".to_owned()) }
            },
            &[Duration::ZERO, Duration::ZERO],
            Duration::from_secs(1),
            Duration::ZERO,
            cancel,
        )
        .await;
        assert!(outcome.unwrap_err().contains("registrar_http_500"));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn engine_start_stops_when_cancelled() {
        let (sender, cancel) = watch::channel(false);
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let outcome = start_with_retry(
            move || {
                let first = counter.fetch_add(1, Ordering::SeqCst) == 0;
                let sender = sender.clone();
                async move {
                    if first {
                        let _ = sender.send(true);
                    }
                    Err::<u32, _>("registrar_http_500".to_owned())
                }
            },
            &[Duration::from_secs(30)],
            Duration::from_secs(1),
            Duration::ZERO,
            cancel,
        )
        .await;
        assert_eq!(outcome, Err(START_CANCELLED.to_owned()));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_hanging_attempt_times_out() {
        let (_sender, cancel) = watch::channel(false);
        let outcome = start_with_retry(
            || async { std::future::pending::<Result<u32, String>>().await },
            &[],
            Duration::from_millis(20),
            Duration::ZERO,
            cancel,
        )
        .await;
        assert!(outcome.unwrap_err().contains("timed out"));
    }

    #[test]
    fn jitter_stays_within_its_bound() {
        let base = Duration::from_millis(10);
        let jitter = Duration::from_millis(5);
        for _ in 0..16 {
            let delay = jittered_delay(base, jitter);
            assert!(delay >= base && delay <= base + jitter);
        }
        assert_eq!(jittered_delay(base, Duration::ZERO), base);
    }

    #[test]
    fn a_poisoned_mic_lock_does_not_break_later_mic_changes() {
        let launcher = test_launcher();
        let mic = &launcher.default_mic;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mic.lock().unwrap();
            panic!("Poisoning the lock");
        }));
        let changed = MicSettings {
            noise_suppression: calling::NoiseSuppression::Off,
            music_mode: true,
        };
        launcher.set_default_mic(changed);
        assert_eq!(
            *launcher
                .default_mic
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            changed
        );
    }

    struct AlwaysFailingTransport;

    impl Transport for AlwaysFailingTransport {
        fn check(&self) -> session::BoxFuture<'_, session::Result<()>> {
            Box::pin(async { Err(session::Error::NoAppTab) })
        }

        fn open(
            &self,
            _app: session::App,
            _with_events: bool,
        ) -> session::BoxFuture<'_, session::Result<Option<session::OpenedTab>>> {
            Box::pin(async { Err(session::Error::NoAppTab) })
        }

        fn wake(&self, _app: session::App) -> session::BoxFuture<'_, session::Result<()>> {
            Box::pin(async { Err(session::Error::NoAppTab) })
        }

        fn diagnose(&self) -> session::BoxFuture<'_, session::Result<session::Diagnosis>> {
            Box::pin(async { Err(session::Error::NoAppTab) })
        }
    }

    #[tokio::test]
    async fn a_transport_that_always_fails_reaches_a_terminal_state() {
        let launcher = CallLauncher::start_with(
            Arc::new(AlwaysFailingTransport),
            false,
            |_| {},
            &[Duration::ZERO, Duration::ZERO],
            Duration::from_secs(1),
            Duration::ZERO,
        );
        let mut state = launcher.state.subscribe();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !matches!(&*state.borrow(), LaunchState::Failed(_)) {
                state.changed().await.expect("launcher state stays open");
            }
        })
        .await
        .expect("start reaches a terminal state");
        assert!(matches!(&*launcher.state.borrow(), LaunchState::Failed(_)));
        assert!(
            wait_until_ready(launcher.engine.subscribe(), Duration::from_millis(20))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn waiting_resolves_once_value_arrives() {
        let (sender, receiver) = watch::channel(None);
        let waiter = tokio::spawn(wait_until_ready(receiver, Duration::from_secs(5)));
        sender.send_replace(Some(7u32));
        assert_eq!(waiter.await.unwrap(), Some(7));
    }

    #[tokio::test]
    async fn waiting_returns_immediately_when_already_ready() {
        let (_sender, receiver) = watch::channel(Some(3u32));
        assert_eq!(wait_until_ready(receiver, Duration::ZERO).await, Some(3));
    }

    #[tokio::test]
    async fn waiting_gives_up_after_the_timeout() {
        let (_sender, receiver) = watch::channel(None::<u32>);
        assert_eq!(
            wait_until_ready(receiver, Duration::from_millis(20)).await,
            None
        );
    }

    #[tokio::test]
    async fn waiting_gives_up_when_the_launcher_is_gone() {
        let (sender, receiver) = watch::channel(None::<u32>);
        drop(sender);
        assert_eq!(
            wait_until_ready(receiver, Duration::from_secs(5)).await,
            None
        );
    }

    #[tokio::test]
    async fn queued_call_fails_with_notice_after_the_timeout() {
        let (_sender, receiver) = watch::channel(None);
        let (mut handle, control) = call_channel();
        queue_outgoing(
            receiver,
            Duration::from_millis(20),
            CallSpec::Echo,
            OutgoingKind::Audio,
            control,
        )
        .await;
        assert_eq!(
            handle.updates.recv().await,
            Some(CallUpdate::State(CallState::Ended {
                reason: EndReason::Failed(NOT_READY.to_owned())
            }))
        );
    }

    #[tokio::test]
    async fn queued_call_is_cancelled_by_hangup() {
        let (_sender, receiver) = watch::channel(None);
        let (mut handle, control) = call_channel();
        handle.commands.send(CallCommand::Hangup).unwrap();
        queue_outgoing(
            receiver,
            Duration::from_secs(30),
            CallSpec::Echo,
            OutgoingKind::Video,
            control,
        )
        .await;
        assert_eq!(
            handle.updates.recv().await,
            Some(CallUpdate::State(CallState::Ended {
                reason: EndReason::Cancelled
            }))
        );
    }
}
