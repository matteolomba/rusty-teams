//! System-wide idle time on Linux: Wayland `ext-idle-notify-v1`, else the X11 screensaver
//! extension. `None` where neither is offered, as before.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

/// The compositor reports idle only after this much quiet, so it is added back on.
const NOTIFY_AFTER: Duration = Duration::from_secs(5);
const INPUT_IDLE_VERSION: u32 = 2;

#[derive(Default)]
struct WaylandIdle {
    available: bool,
    idle_since: Option<Instant>,
    /// A watcher is still starting or running; avoids one thread per poll.
    starting: bool,
}

pub fn system_idle() -> Option<Duration> {
    resolve_idle(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        wayland_idle,
        x11_idle,
    )
}

/// Wayland wins when usable; a missing or failed Wayland watcher falls back to X11.
fn resolve_idle(
    wayland_display: bool,
    wayland: impl FnOnce() -> Option<Duration>,
    x11: impl FnOnce() -> Option<Duration>,
) -> Option<Duration> {
    if wayland_display {
        if let Some(idle) = wayland() {
            return Some(idle);
        }
    }
    x11()
}

fn wayland_idle() -> Option<Duration> {
    static STATE: LazyLock<Arc<Mutex<WaylandIdle>>> =
        LazyLock::new(|| Arc::new(Mutex::new(WaylandIdle::default())));
    wayland_idle_from(&*STATE, start_wayland_watcher)
}

/// Reads the shared watcher state, (re)starting it after a failed attempt.
fn wayland_idle_from(
    state: &Arc<Mutex<WaylandIdle>>,
    start: impl Fn(&Arc<Mutex<WaylandIdle>>) -> bool,
) -> Option<Duration> {
    let state = Arc::clone(state);
    {
        let mut guard = state.lock().ok()?;
        if !guard.available && !guard.starting {
            guard.starting = true;
            drop(guard);
            if !start(&state) {
                if let Ok(mut guard) = state.lock() {
                    guard.starting = false;
                }
                return None;
            }
        }
    }
    let guard = state.lock().ok()?;
    guard
        .available
        .then(|| idle_duration(guard.idle_since, Instant::now()))
}

fn start_wayland_watcher(state: &Arc<Mutex<WaylandIdle>>) -> bool {
    let watched = Arc::clone(state);
    std::thread::Builder::new()
        .name("wayland-idle".into())
        .spawn(move || {
            let usable = watch_wayland(&watched);
            if let Ok(mut guard) = watched.lock() {
                guard.starting = false;
                if !usable {
                    guard.available = false;
                    guard.idle_since = None;
                }
            }
        })
        .is_ok()
}

pub fn idle_duration(idle_since: Option<Instant>, now: Instant) -> Duration {
    idle_since.map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
}

struct Watcher {
    state: Arc<Mutex<WaylandIdle>>,
}

/// Runs the Wayland event loop. `false` means the watcher must be retried.
fn watch_wayland(state: &Arc<Mutex<WaylandIdle>>) -> bool {
    let Ok(connection) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, mut queue)) = registry_queue_init::<Watcher>(&connection) else {
        return false;
    };
    let handle = queue.handle();
    let seat: wl_seat::WlSeat = match globals.bind(&handle, 1..=1, ()) {
        Ok(seat) => seat,
        Err(_) => return false,
    };
    let notifier: ExtIdleNotifierV1 = match globals.bind(&handle, 1..=INPUT_IDLE_VERSION, ()) {
        Ok(notifier) => notifier,
        Err(_) => return false,
    };
    let timeout = NOTIFY_AFTER.as_millis() as u32;
    // Version 2 counts input only, like Windows' last-input time; version 1 also honours
    // idle inhibitors such as a playing video.
    let _notification = if notifier.version() >= INPUT_IDLE_VERSION {
        notifier.get_input_idle_notification(timeout, &seat, &handle, ())
    } else {
        notifier.get_idle_notification(timeout, &seat, &handle, ())
    };
    let Ok(mut guard) = state.lock() else {
        return false;
    };
    guard.available = true;
    drop(guard);
    let mut watcher = Watcher {
        state: Arc::clone(state),
    };
    while queue.blocking_dispatch(&mut watcher).is_ok() {}
    false
}

impl Dispatch<ExtIdleNotificationV1, ()> for Watcher {
    fn event(
        watcher: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Ok(mut state) = watcher.state.lock() else {
            return;
        };
        match event {
            ext_idle_notification_v1::Event::Idled => {
                state.idle_since = Instant::now().checked_sub(NOTIFY_AFTER);
            }
            ext_idle_notification_v1::Event::Resumed => state.idle_since = None,
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Watcher {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Watcher {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for Watcher {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn x11_idle() -> Option<Duration> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::screensaver::ConnectionExt as _;

    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let info = connection.screensaver_query_info(root).ok()?.reply().ok()?;
    Some(Duration::from_millis(u64::from(info.ms_since_user_input)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    #[test]
    fn idle_counts_from_the_reported_start_and_is_zero_while_active() {
        let start = Instant::now();
        assert_eq!(idle_duration(None, start), Duration::ZERO);
        let now = start + Duration::from_secs(400);
        assert_eq!(idle_duration(Some(start), now), Duration::from_secs(400));
    }

    #[test]
    fn missing_wayland_falls_back_to_x11() {
        assert_eq!(
            resolve_idle(true, || None, || Some(seconds(7))),
            Some(seconds(7))
        );
        assert_eq!(
            resolve_idle(true, || Some(seconds(3)), || Some(seconds(7))),
            Some(seconds(3))
        );
        assert_eq!(
            resolve_idle(false, || Some(seconds(3)), || Some(seconds(7))),
            Some(seconds(7))
        );
        assert_eq!(resolve_idle(false, || None, || None), None);
    }

    #[test]
    fn wayland_watcher_is_retried_until_ready() {
        let state = Arc::new(Mutex::new(WaylandIdle::default()));
        let attempts = std::cell::Cell::new(0);
        let start = |state: &Arc<Mutex<WaylandIdle>>| {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                return false;
            }
            let Ok(mut guard) = state.lock() else {
                return false;
            };
            guard.starting = false;
            guard.available = true;
            true
        };
        assert_eq!(wayland_idle_from(&state, &start), None);
        assert_eq!(wayland_idle_from(&state, &start), None);
        assert_eq!(attempts.get(), 2);
        assert!(wayland_idle_from(&state, &start).is_some());
        assert!(wayland_idle_from(&state, &start).is_some());
        assert_eq!(attempts.get(), 3);
    }
}
