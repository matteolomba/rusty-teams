use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::Value;
use session::{App, Error, TabCommand, TabEvent, is_login_url};
use tokio::sync::watch;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DevToolsProtocolEventReceivedEventHandler, NavigationCompletedEventHandler,
    ProcessFailedEventHandler, SourceChangedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HRESULT, HSTRING, Interface, PCWSTR, PWSTR, w};

use crate::cdp_failure::{CallContext, failure_reason, tab_state};
use crate::fanout::Subscribers;
use crate::guards::{Deferred, EventSubscriptions, InitGate, InitMessage};
use crate::host_dialog::{self, Dialog};
use crate::park::{ParkDecision, ParkGate};
use crate::sanitize::{ENV_OS_SSO, env_flag};
use crate::sleep::{Activity, CallTally, Sleep, blocker, call_label};
use crate::transport::{HostState, READY_WAIT, UiRequest, WebViewTransport};
use crate::{BROWSER_ARGUMENTS, FORWARDED_EVENTS};

const WINDOW_CLASS: PCWSTR = w!("RustyTeamsWebView");
const WM_DRAIN: u32 = WM_APP;
const WM_RESTART: u32 = WM_APP + 1;
const LOGIN_TIMER: usize = 1;
const LOGIN_SHOW_DELAY_MS: u32 = 4000;
const PARK_TIMER: usize = 2;
const PARK_DELAY_MS: u32 = 60_000;
const SLEEP_TIMER: usize = 3;
const SLEEP_CHECK_MS: u32 = 60_000;
const BUSY_REPORT_EVERY: u32 = 10;
const LOGIN_PROMPT_INTERVAL: Duration = Duration::from_secs(600);
const WINDOW_WIDTH: i32 = 1100;
const WINDOW_HEIGHT: i32 = 800;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub user_data_folder: PathBuf,
    pub window_title: String,
    pub log: fn(&str),
}

struct View {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    subscriptions: EventSubscriptions,
}

pub(crate) struct Shared {
    config: HostConfig,
    windows: HashMap<App, HWND>,
    views: RefCell<HashMap<App, View>>,
    pub(crate) environment: RefCell<Option<ICoreWebView2Environment>>,
    pub(crate) dialogs: RefCell<HashMap<u64, Dialog>>,
    subscribers: RefCell<Subscribers>,
    park_gate: RefCell<ParkGate>,
    state: watch::Sender<HostState>,
    requests: mpsc::Receiver<UiRequest>,
    restarting: Cell<bool>,
    init_gate: InitGate,
    last_login_prompt: Cell<Option<Instant>>,
    sleep: RefCell<Sleep<TabCommand>>,
    sleep_blocker: Cell<Option<&'static str>>,
    calls: RefCell<CallTally>,
    busy_checks: Cell<u32>,
}

thread_local! {
    static SHARED: RefCell<Option<Rc<Shared>>> = const { RefCell::new(None) };
}

pub(crate) fn shared() -> Option<Rc<Shared>> {
    SHARED.with(|slot| slot.borrow().clone())
}

/// Starts the WebView2 UI thread; the transport answers once both apps are loaded.
pub fn start(config: HostConfig) -> Arc<WebViewTransport> {
    let (requests_sender, requests) = mpsc::channel();
    let (state_sender, state) = watch::channel(HostState::default());
    let (window_sender, window_receiver) = mpsc::channel::<isize>();
    let failure_state = state_sender.clone();
    let spawned = std::thread::Builder::new()
        .name("webview2".into())
        .spawn(move || run(config, requests, state_sender, window_sender));
    if let Err(error) = spawned {
        failure_state
            .send_modify(|state| state.failure = Some(format!("no webview thread: {error}")));
    }
    let window = match window_receiver.recv_timeout(READY_WAIT) {
        Ok(raw) => Some(raw),
        Err(_) => {
            failure_state.send_modify(|state| {
                if state.failure.is_none() {
                    state.failure = Some("webview window did not start in time".into());
                }
            });
            None
        }
    };
    let waker = Arc::new(move || {
        if let Some(raw) = window {
            // SAFETY: `raw` round-trips the HWND sent by the UI thread; PostMessageW only copies
            // handle/value arguments and reports failure if the window is already gone.
            unsafe {
                let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_DRAIN, WPARAM(0), LPARAM(0));
            }
        }
    });
    Arc::new(WebViewTransport::new(requests_sender, waker, state))
}

fn run(
    config: HostConfig,
    requests: mpsc::Receiver<UiRequest>,
    state: watch::Sender<HostState>,
    window_sender: mpsc::Sender<isize>,
) {
    let fail = |reason: String| state.send_modify(|state| state.failure = Some(reason));
    // SAFETY: this is a freshly spawned thread with no COM initialised, and every COM call below
    // runs on it only after this succeeds.
    if let Err(error) = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok() {
        return fail(format!("COM: {error}"));
    }
    let windows = match create_windows(&config.window_title) {
        Ok(windows) => windows,
        Err(error) => return fail(format!("window: {error}")),
    };
    let _ = window_sender.send(windows[&App::Teams].0 as isize);
    let shared = Rc::new(Shared {
        config,
        windows,
        views: RefCell::new(HashMap::new()),
        environment: RefCell::new(None),
        dialogs: RefCell::new(HashMap::new()),
        subscribers: RefCell::new(Subscribers::default()),
        park_gate: RefCell::new(ParkGate::default()),
        state,
        requests,
        restarting: Cell::new(false),
        init_gate: InitGate::default(),
        last_login_prompt: Cell::new(None),
        sleep: RefCell::new(Sleep::default()),
        sleep_blocker: Cell::new(None),
        calls: RefCell::new(CallTally::default()),
        busy_checks: Cell::new(0),
    });
    SHARED.with(|slot| *slot.borrow_mut() = Some(shared.clone()));
    load(&shared);
    // SAFETY: the HWND was created on this thread and stays valid throughout this message loop;
    // the timer ID belongs to this window.
    unsafe {
        SetTimer(
            Some(shared.windows[&App::Teams]),
            SLEEP_TIMER,
            SLEEP_CHECK_MS,
            None,
        )
    };
    let mut message = MSG::default();
    // SAFETY: `message` is a live default-initialized MSG out-param and the filter accepts all
    // windows, so GetMessageW can write it.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        // SAFETY: `message` is the live MSG filled by the GetMessageW call above; dispatching it
        // delivers a valid message to the window procedure.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn create_windows(title: &str) -> windows::core::Result<HashMap<App, HWND>> {
    // SAFETY: no pointers are passed in; a module handle is valid for the process lifetime.
    let instance = HINSTANCE(unsafe { GetModuleHandleW(None) }?.0);
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_procedure),
        hInstance: instance,
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is a live WNDCLASSW with a static class name and a valid window procedure;
    // the registration stays valid for the process lifetime.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::new(
            HRESULT(-1),
            "cannot register the webview window class",
        ));
    }
    let title = HSTRING::from(title);
    let mut windows = HashMap::new();
    for app in App::ALL {
        // SAFETY: WINDOW_CLASS was registered above, `instance` is the process module handle, and
        // `title` is a live HSTRING for the duration of the call.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                WINDOW_CLASS,
                &title,
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
                None,
                None,
                Some(instance),
                None,
            )
        }?;
        windows.insert(app, window);
    }
    Ok(windows)
}

fn load(shared: &Rc<Shared>) {
    shared.init_gate.begin();
    let outcome = create_views(shared, |app| app.start_url());
    let deferred = shared.init_gate.end();
    match outcome {
        Ok(()) => shared.state.send_modify(|state| {
            state.ready = true;
            state.failure = None;
        }),
        Err(reason) => shared.state.send_modify(|state| {
            state.ready = false;
            state.failure = Some(reason);
        }),
    }
    post_deferred(shared, deferred);
}

fn post_deferred(shared: &Shared, deferred: Deferred) {
    let window = shared.windows[&App::Teams];
    for message in [
        deferred.restart.then_some(WM_RESTART),
        deferred.drain.then_some(WM_DRAIN),
    ]
    .into_iter()
    .flatten()
    {
        // SAFETY: `window` is an HWND created on this UI thread and still alive; posting only
        // copies handle/value arguments and ignores failure.
        unsafe {
            let _ = PostMessageW(Some(window), message, WPARAM(0), LPARAM(0));
        }
    }
}

fn create_views(
    shared: &Rc<Shared>,
    first_url: impl Fn(App) -> &'static str,
) -> std::result::Result<(), String> {
    let environment = create_environment(&shared.config.user_data_folder)?;
    *shared.environment.borrow_mut() = Some(environment.clone());
    for app in App::ALL {
        let view = create_view(shared, &environment, app, first_url(app))
            .map_err(|error| format!("{app} webview: {error}"))?;
        shared.views.borrow_mut().insert(app, view);
    }
    Ok(())
}

pub(crate) fn environment_options() -> ICoreWebView2EnvironmentOptions {
    let options = CoreWebView2EnvironmentOptions::default();
    let sso = std::env::var(ENV_OS_SSO).ok();
    // SAFETY: `options` is a live owned COM wrapper; the option values are passed by value and
    // remain valid for the call.
    unsafe {
        options.set_additional_browser_arguments(BROWSER_ARGUMENTS.to_owned());
        options.set_allow_single_sign_on_using_os_primary_account(env_flag(sso.as_deref()));
    }
    options.into()
}

fn create_environment(
    user_data_folder: &std::path::Path,
) -> std::result::Result<ICoreWebView2Environment, String> {
    let options = environment_options();
    let folder = HSTRING::from(user_data_folder.as_os_str());
    let (sender, receiver) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        // SAFETY: called on the COM-initialized UI thread; `folder` and `options` stay alive for
        // the call, and `handler` keeps its channel sender alive until the callback runs.
        Box::new(move |handler| unsafe {
            CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &folder, &options, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, environment| {
            code?;
            let _ = sender.send(environment);
            Ok(())
        }),
    )
    .map_err(|error| format!("WebView2 runtime: {error}"))?;
    receiver
        .recv_timeout(READY_WAIT)
        .map_err(|_| "WebView2 runtime did not answer in time".to_owned())?
        .ok_or_else(|| "WebView2 runtime returned no environment".to_owned())
}

fn create_view(
    shared: &Rc<Shared>,
    environment: &ICoreWebView2Environment,
    app: App,
    first_url: &str,
) -> windows::core::Result<View> {
    let window = shared.windows[&app];
    let (sender, receiver) = mpsc::channel();
    let creator = environment.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        // SAFETY: `creator` is a live environment, `window` is an HWND created on this thread, and
        // `handler` keeps its channel sender alive until the callback runs.
        Box::new(move |handler| unsafe {
            creator
                .CreateCoreWebView2Controller(window, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, controller| {
            code?;
            let _ = sender.send(controller);
            Ok(())
        }),
    )
    .map_err(|error| match error {
        webview2_com::Error::WindowsError(error) => error,
        other => windows::core::Error::new(windows::core::HRESULT(-1), other.to_string()),
    })?;
    let controller = receiver
        .recv_timeout(READY_WAIT)
        .map_err(|_| windows::core::Error::new(HRESULT(-1), "no controller in time"))?
        .ok_or_else(|| windows::core::Error::new(HRESULT(-1), "no controller"))?;

    // SAFETY: `controller` is a live owned controller from the async creation above; the webview
    // returned here is an owned COM pointer.
    let webview = unsafe { controller.CoreWebView2() }?;
    fit(window, &controller);
    // SAFETY: `controller` and `webview` are live owned pointers; the settings interface returned
    // by Settings() is used only within this statement.
    unsafe {
        controller.SetIsVisible(true)?;
        webview.Settings()?.SetIsStatusBarEnabled(false)?;
    }
    let mut subscriptions = EventSubscriptions::default();
    watch_source(&webview, app, &mut subscriptions)?;
    watch_loads(&webview, app, &mut subscriptions)?;
    watch_failures(&webview, app, &mut subscriptions)?;
    forward_events(&webview, app, &mut subscriptions)?;
    // SAFETY: `webview` is live and the temporary HSTRING stays alive until Navigate returns.
    unsafe { webview.Navigate(&HSTRING::from(first_url)) }?;
    Ok(View {
        controller,
        webview,
        subscriptions,
    })
}

fn watch_source(
    webview: &ICoreWebView2,
    app: App,
    subscriptions: &mut EventSubscriptions,
) -> windows::core::Result<()> {
    let mut token = 0;
    let handler = SourceChangedEventHandler::create(Box::new(move |sender, _| {
        if let (Some(sender), Some(shared)) = (sender, shared()) {
            on_source_changed(&shared, app, source_of(&sender));
        }
        Ok(())
    }));
    // SAFETY: `webview` is live, `handler` is a valid COM handler held by the webview, and `token`
    // lives in the caller's subscription until removal.
    unsafe { webview.add_SourceChanged(&handler, &mut token) }?;
    let source = webview.clone();
    // SAFETY: `source` is an owned clone of the live webview and `token` is the registration token
    // returned by add_SourceChanged, still valid when this cleanup runs.
    subscriptions.add(move || unsafe {
        let _ = source.remove_SourceChanged(token);
    });
    Ok(())
}

fn watch_loads(
    webview: &ICoreWebView2,
    app: App,
    subscriptions: &mut EventSubscriptions,
) -> windows::core::Result<()> {
    let mut token = 0;
    let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, _| {
        let (Some(sender), Some(shared)) = (sender, shared()) else {
            return Ok(());
        };
        let url = current_url(&shared, app);
        let waiting = shared.sleep.borrow_mut().loaded(app);
        for command in waiting {
            call(&shared, app, command);
        }
        if is_running_app(app, &url) {
            // SAFETY: the HWND comes from `shared.windows` on this UI thread; the WM_TIMER handler
            // kills this timer before it acts.
            unsafe { SetTimer(Some(shared.windows[&app]), PARK_TIMER, PARK_DELAY_MS, None) };
            if shared.park_gate.borrow().held(app) {
                set_memory_level(&sender, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
                release_memory(&sender);
            }
        } else if app.is_park_url(&url) {
            release_memory(&sender);
        }
        Ok(())
    }));
    // SAFETY: `webview` is live, `handler` is a valid COM handler held by the webview, and `token`
    // lives in the caller's subscription until removal.
    unsafe { webview.add_NavigationCompleted(&handler, &mut token) }?;
    let source = webview.clone();
    // SAFETY: `source` is an owned clone of the live webview and `token` is the registration token
    // returned by add_NavigationCompleted, still valid when this cleanup runs.
    subscriptions.add(move || unsafe {
        let _ = source.remove_NavigationCompleted(token);
    });
    Ok(())
}

fn is_running_app(app: App, url: &str) -> bool {
    app.origins().iter().any(|origin| url.starts_with(origin)) && !app.is_park_url(url)
}

fn park(shared: &Shared, app: App) {
    if let Some(webview) = webview_of(shared, app)
        && is_running_app(app, &current_url(shared, app))
    {
        set_memory_level(&webview, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
        // SAFETY: `webview` is the live owned webview from webview_of and the park URL HSTRING
        // stays alive until Navigate returns.
        unsafe {
            let _ = webview.Navigate(&HSTRING::from(app.park_url()));
        }
    }
}

/// A held tab stays loaded for the whole call, so its renderer is told to shrink instead of being parked.
fn hold(shared: &Rc<Shared>, app: App, held: bool) {
    if held {
        wake_views(shared, None);
    }
    shared.park_gate.borrow_mut().hold(app, held);
    if held
        && let Some(webview) = webview_of(shared, app)
        && is_running_app(app, &current_url(shared, app))
    {
        set_memory_level(&webview, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
        release_memory(&webview);
    }
}

fn set_memory_level(webview: &ICoreWebView2, level: COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL) {
    if let Ok(webview) = webview.cast::<ICoreWebView2_19>() {
        // SAFETY: a successful cast yields a live ICoreWebView2_19 from the owned webview; the
        // level is a plain enum value.
        unsafe {
            let _ = webview.SetMemoryUsageTargetLevel(level);
        }
    }
}

// The park page stays same-site, so the renderer keeps the app heap until it is told to shrink.
fn release_memory(webview: &ICoreWebView2) {
    let ignore = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
    // SAFETY: `webview` is live; both JSON arguments are static strings and the local completion
    // handler stays alive until the call completes.
    unsafe {
        let _ = webview.CallDevToolsProtocolMethod(
            w!("Memory.simulatePressureNotification"),
            w!(r#"{"level":"critical"}"#),
            &ignore,
        );
    }
}

fn watch_failures(
    webview: &ICoreWebView2,
    app: App,
    subscriptions: &mut EventSubscriptions,
) -> windows::core::Result<()> {
    let mut token = 0;
    let handler = ProcessFailedEventHandler::create(Box::new(move |sender, arguments| {
        let (Some(sender), Some(arguments), Some(shared)) = (sender, arguments, shared()) else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
        // SAFETY: `arguments` is a live COM interface reference from this callback and `kind` is a
        // live out-param.
        unsafe { arguments.ProcessFailedKind(&mut kind) }?;
        if shared.sleep.borrow().asleep() {
            return Ok(());
        }
        if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
            if !shared.restarting.replace(true) {
                // SAFETY: the HWND comes from `shared.windows` on this UI thread; PostMessageW only
                // copies arguments and reports failure if the window is gone.
                unsafe {
                    PostMessageW(Some(shared.windows[&app]), WM_RESTART, WPARAM(0), LPARAM(0))
                }?;
            }
        } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
            || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
        {
            // SAFETY: `sender` is the live ICoreWebView2 reference supplied with this callback.
            unsafe { sender.Reload() }?;
        }
        Ok(())
    }));
    // SAFETY: `webview` is live, `handler` is a valid COM handler held by the webview, and `token`
    // remains alive in the subscription until removal.
    unsafe { webview.add_ProcessFailed(&handler, &mut token) }?;
    let source = webview.clone();
    // SAFETY: `source` is an owned clone of the live webview and `token` is the token returned by
    // add_ProcessFailed, still valid at cleanup time.
    subscriptions.add(move || unsafe {
        let _ = source.remove_ProcessFailed(token);
    });
    Ok(())
}

fn forward_events(
    webview: &ICoreWebView2,
    app: App,
    subscriptions: &mut EventSubscriptions,
) -> windows::core::Result<()> {
    for name in FORWARDED_EVENTS {
        // SAFETY: `webview` is live and the event-name HSTRING stays alive for the call; the
        // receiver returned is an owned COM pointer.
        let receiver = unsafe { webview.GetDevToolsProtocolEventReceiver(&HSTRING::from(name)) }?;
        let mut token = 0;
        let handler =
            DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, arguments| {
                let (Some(arguments), Some(shared)) = (arguments, shared()) else {
                    return Ok(());
                };
                let mut parameters = PWSTR::null();
                // SAFETY: `arguments` is a live callback reference and `parameters` is a live
                // PWSTR out-param consumed by take_pwstr.
                unsafe { arguments.ParameterObjectAsJson(&mut parameters) }?;
                let event = TabEvent {
                    method: name.to_owned(),
                    params: serde_json::from_str(&take_pwstr(parameters)).unwrap_or(Value::Null),
                };
                shared.subscribers.borrow_mut().publish(app, &event);
                Ok(())
            }));
        // SAFETY: `receiver` is a live owned pointer, `handler` is a valid COM handler held by the
        // receiver, and `token` stays alive in the subscription until removal.
        unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }?;
        // SAFETY: `receiver` is owned by this closure and `token` is the token returned by the
        // matching add call.
        subscriptions.add(move || unsafe {
            let _ = receiver.remove_DevToolsProtocolEventReceived(token);
        });
    }
    Ok(())
}

fn source_of(webview: &ICoreWebView2) -> String {
    let mut source = PWSTR::null();
    // SAFETY: `webview` is live and `source` is a live PWSTR out-param consumed by take_pwstr.
    match unsafe { webview.Source(&mut source) } {
        Ok(()) => take_pwstr(source),
        Err(_) => String::new(),
    }
}

fn on_source_changed(shared: &Shared, app: App, url: String) {
    let window = shared.windows[&app];
    let login = is_login_url(&url);
    let on_app = app.origins().iter().any(|origin| url.starts_with(origin));
    shared.state.send_modify(|state| {
        state.urls.insert(app, url);
    });
    // SAFETY: `window` is a live HWND from `shared.windows` on this UI thread; the calls take the
    // handle by value, and the login timer is killed once it is no longer needed.
    unsafe {
        if login {
            SetTimer(Some(window), LOGIN_TIMER, LOGIN_SHOW_DELAY_MS, None);
        } else if on_app {
            let _ = KillTimer(Some(window), LOGIN_TIMER);
            let _ = ShowWindow(window, SW_HIDE);
        }
    }
}

fn webview_of(shared: &Shared, app: App) -> Option<ICoreWebView2> {
    shared
        .views
        .borrow()
        .get(&app)
        .map(|view| view.webview.clone())
}

fn current_url(shared: &Shared, app: App) -> String {
    shared
        .state
        .borrow()
        .urls
        .get(&app)
        .cloned()
        .unwrap_or_default()
}

fn drain(shared: &Rc<Shared>) {
    while let Ok(request) = shared.requests.try_recv() {
        match request {
            UiRequest::Call { app, command } => call(shared, app, command),
            UiRequest::Subscribe { app, sink } => shared.subscribers.borrow_mut().add(app, sink),
            UiRequest::Wake { app } => wake(shared, app),
            UiRequest::Hold { app, held } => hold(shared, app, held),
            UiRequest::Retry => {
                if !shared.state.borrow().ready && !shared.restarting.replace(true) {
                    restart(shared);
                }
            }
            UiRequest::OpenDialog { id, spec, events } => {
                wake_views(shared, None);
                host_dialog::open(shared, id, spec, events)
            }
            UiRequest::Dialog { id, command } => host_dialog::run(shared, id, command),
        }
    }
}

fn call(shared: &Rc<Shared>, app: App, command: TabCommand) {
    shared.sleep.borrow_mut().touch(Instant::now());
    shared.calls.borrow_mut().note(format!(
        "{app} {}",
        call_label(&command.method, &command.params)
    ));
    wake_views(shared, None);
    let Some(command) = shared.sleep.borrow_mut().admit(app, command) else {
        return;
    };
    let Some(webview) = webview_of(shared, app) else {
        return command.respond(Err(Error::Cdp("webview is restarting".into())));
    };
    let parameters_json = command.params.to_string();
    let parameters_bytes = parameters_json.len();
    let method = HSTRING::from(command.method.as_str());
    let parameters = HSTRING::from(parameters_json);
    let pending = Rc::new(RefCell::new(Some(command)));
    let answered = pending.clone();
    shared.park_gate.borrow_mut().command_started(app);
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |code, json| {
            if let Some(command) = answered.borrow_mut().take() {
                let result = match code {
                    Ok(()) => Ok(serde_json::from_str(&json).unwrap_or(Value::Null)),
                    Err(error) => Err(Error::Cdp(describe_failure(
                        app,
                        &command.method,
                        parameters_bytes,
                        &error.to_string(),
                        &json,
                    ))),
                };
                if let Some(shared) = self::shared() {
                    shared.park_gate.borrow_mut().command_finished(app);
                }
                command.respond(result);
            }
            Ok(())
        }));
    if let Err(error) =
        // SAFETY: `webview` is a live owned pointer; `method`, `parameters` and `handler` stay alive
        // until the call returns, and the handler owns the pending command.
        unsafe { webview.CallDevToolsProtocolMethod(&method, &parameters, &handler) }
        && let Some(command) = pending.borrow_mut().take()
    {
        let reason = describe_failure(
            app,
            &command.method,
            parameters_bytes,
            &error.to_string(),
            "",
        );
        shared.park_gate.borrow_mut().command_finished(app);
        command.respond(Err(Error::Cdp(reason)));
    }
}

fn describe_failure(
    app: App,
    method: &str,
    parameters_bytes: usize,
    hresult: &str,
    json: &str,
) -> String {
    let (tab, in_flight) = match shared() {
        Some(shared) => (
            tab_state(app, &current_url(&shared, app)),
            shared.park_gate.borrow().in_flight(app),
        ),
        None => ("unknown", 0),
    };
    let context = CallContext {
        method,
        params_bytes: parameters_bytes,
        tab,
        in_flight,
    };
    failure_reason(&context, hresult, json)
}

fn wake(shared: &Rc<Shared>, app: App) {
    if shared.sleep.borrow().asleep() {
        wake_views(shared, Some(app));
        return;
    }
    let Some(webview) = webview_of(shared, app) else {
        return;
    };
    let url = current_url(shared, app);
    if is_login_url(&url) {
        prompt_login(shared, app);
        return;
    }
    set_memory_level(&webview, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL);
    // SAFETY: `webview` is live and any temporary HSTRING for the URL lives until the call returns.
    unsafe {
        let _ = if is_running_app(app, &url) {
            webview.Reload()
        } else {
            webview.Navigate(&HSTRING::from(app.start_url()))
        };
    }
}

fn prompt_login(shared: &Shared, app: App) {
    let recently = shared
        .last_login_prompt
        .get()
        .is_some_and(|shown| shown.elapsed() < LOGIN_PROMPT_INTERVAL);
    // SAFETY: `shared.windows[&app]` is a live HWND on this UI thread; IsWindowVisible does not
    // dereference the handle.
    let visible = unsafe { IsWindowVisible(shared.windows[&app]) }.as_bool();
    if !recently && !visible {
        shared.last_login_prompt.set(Some(Instant::now()));
        show(shared, app);
    }
}

fn show(shared: &Shared, app: App) {
    let window = shared.windows[&app];
    // SAFETY: `window` is a live HWND from `shared.windows` on this UI thread.
    unsafe {
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
    }
    if let Some(view) = shared.views.borrow().get(&app) {
        fit(window, &view.controller);
    }
}

fn close_views(shared: &Rc<Shared>) {
    let views: Vec<View> = shared
        .views
        .borrow_mut()
        .drain()
        .map(|(_, view)| view)
        .collect();
    for mut view in views {
        view.subscriptions.clear();
        // SAFETY: each view was drained from `shared.views` and its subscriptions cleared first, so
        // `controller` is a live owned pointer that is not used after closing.
        unsafe {
            let _ = view.controller.Close();
        }
    }
}

fn restart(shared: &Rc<Shared>) {
    shared.state.send_modify(|state| state.ready = false);
    let stranded = shared.sleep.borrow_mut().reset();
    for command in stranded {
        command.respond(Err(Error::Cdp("webview is restarting".into())));
    }
    shared.subscribers.borrow_mut().clear();
    shared.park_gate.borrow_mut().forget_commands();
    host_dialog::close_all(shared);
    close_views(shared);
    load(shared);
    shared.restarting.set(false);
}

/// Both apps parked and unused for a while: the web views close so the browser processes exit.
fn maybe_sleep(shared: &Rc<Shared>) {
    let urls: Vec<String> = App::ALL
        .iter()
        .map(|&app| current_url(shared, app))
        .collect();
    let activity = Activity {
        restarting: shared.restarting.get(),
        all_parked: App::ALL
            .iter()
            .zip(&urls)
            .all(|(app, url)| app.is_park_url(url)),
        login_pending: urls.iter().any(|url| is_login_url(url)),
        held: App::ALL
            .iter()
            .any(|&app| shared.park_gate.borrow().held(app)),
        in_flight: App::ALL
            .iter()
            .any(|&app| shared.park_gate.borrow().in_flight(app) > 0),
        dialogs_open: !shared.dialogs.borrow().is_empty(),
        subscribed: shared.subscribers.borrow_mut().has_live(),
    };
    if !shared.sleep.borrow().quiet(Instant::now()) {
        let checks = shared.busy_checks.get() + 1;
        shared.busy_checks.set(checks % BUSY_REPORT_EVERY);
        if checks == BUSY_REPORT_EVERY && !shared.sleep.borrow().asleep() {
            let summary = shared.calls.borrow_mut().summary();
            (shared.config.log)(&format!(
                "webview: busy, calls in the last {BUSY_REPORT_EVERY} min: {summary}"
            ));
        }
        return;
    }
    shared.busy_checks.set(0);
    let blocked = blocker(activity);
    if blocked != shared.sleep_blocker.replace(blocked)
        && let Some(reason) = blocked
    {
        (shared.config.log)(&format!("webview: stays awake, {reason}"));
    }
    if blocked.is_some() {
        return;
    }
    shared.sleep.borrow_mut().fall_asleep();
    (shared.config.log)("webview: asleep");
    close_views(shared);
    *shared.environment.borrow_mut() = None;
}

/// Brings the web views back at the park page, or `full` at the running app when it was asked to reload.
fn wake_views(shared: &Rc<Shared>, full: Option<App>) {
    if !shared.sleep.borrow_mut().wake() {
        return;
    }
    (shared.config.log)(&match full {
        Some(app) => format!("webview: awake, {app} reloads"),
        None => "webview: awake".to_owned(),
    });
    shared.init_gate.begin();
    let outcome = create_views(shared, |app| {
        if Some(app) == full {
            app.start_url()
        } else {
            app.park_url()
        }
    });
    let deferred = shared.init_gate.end();
    post_deferred(shared, deferred);
    if let Err(reason) = outcome {
        let stranded = shared.sleep.borrow_mut().reset();
        for command in stranded {
            command.respond(Err(Error::Cdp(reason.clone())));
        }
        shared.state.send_modify(|state| {
            state.ready = false;
            state.failure = Some(reason);
        });
    }
}

pub(crate) fn fit(window: HWND, controller: &ICoreWebView2Controller) {
    let mut bounds = RECT::default();
    // SAFETY: `window` is a live HWND and `bounds` is a live RECT out-param passed to
    // GetClientRect; SetBounds receives the filled bounds.
    unsafe {
        if GetClientRect(window, &mut bounds).is_ok() {
            let _ = controller.SetBounds(bounds);
        }
    }
}

fn app_of(shared: &Shared, window: HWND) -> Option<App> {
    shared
        .windows
        .iter()
        .find_map(|(&app, &candidate)| (candidate == window).then_some(app))
}

extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(shared) = shared() else {
        // SAFETY: these are the exact window/message/wparam/lparam values Windows passed to this
        // window procedure; DefWindowProcW only forwards them.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    let initializing = match message {
        WM_DRAIN => Some(InitMessage::Drain),
        WM_RESTART => Some(InitMessage::Restart),
        _ => None,
    };
    if initializing.is_some_and(|message| shared.init_gate.defer(message)) {
        return LRESULT(0);
    }
    match message {
        WM_DRAIN => drain(&shared),
        WM_RESTART => restart(&shared),
        WM_SIZE => {
            if let Some(app) = app_of(&shared, window)
                && let Some(view) = shared.views.borrow().get(&app)
            {
                fit(window, &view.controller);
            }
        }
        WM_TIMER if wparam.0 == LOGIN_TIMER => {
            // SAFETY: `window` is the live HWND delivered with this WM_TIMER message and the timer
            // ID was set on this thread.
            unsafe {
                let _ = KillTimer(Some(window), LOGIN_TIMER);
            }
            if let Some(app) = app_of(&shared, window)
                && is_login_url(&current_url(&shared, app))
            {
                show(&shared, app);
                shared.last_login_prompt.set(Some(Instant::now()));
            }
        }
        WM_TIMER if wparam.0 == SLEEP_TIMER => maybe_sleep(&shared),
        WM_TIMER if wparam.0 == PARK_TIMER => {
            // SAFETY: `window` is the live HWND delivered with this WM_TIMER message; PARK_TIMER is
            // owned by this thread's message loop.
            unsafe {
                let _ = KillTimer(Some(window), PARK_TIMER);
            }
            if let Some(app) = app_of(&shared, window) {
                let decision = shared.park_gate.borrow_mut().decide(app);
                match decision {
                    ParkDecision::Park => park(&shared, app),
                    // SAFETY: `window` is the live HWND from this WM_TIMER message; re-arming the
                    // same timer ID is safe on this thread.
                    ParkDecision::Wait => unsafe {
                        SetTimer(Some(window), PARK_TIMER, PARK_DELAY_MS, None);
                    },
                }
            }
        }
        // SAFETY: `window` is the live HWND delivered with WM_CLOSE; hiding rather than destroying
        // it keeps the window map valid for this thread.
        WM_CLOSE => unsafe {
            let _ = ShowWindow(window, SW_HIDE);
        },
        // SAFETY: the arguments are the exact values Windows passed to this window procedure;
        // DefWindowProcW only forwards them.
        _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
    LRESULT(0)
}
