use std::path::PathBuf;

#[cfg(windows)]
pub fn install(directory: PathBuf, crash_log: PathBuf) {
    windows_impl::install(directory, crash_log);
}

#[cfg(not(windows))]
pub fn install(_directory: PathBuf, _crash_log: PathBuf) {}

#[cfg(windows)]
pub fn heartbeat() {
    windows_impl::heartbeat();
}

#[cfg(not(windows))]
pub fn heartbeat() {}

#[cfg(any(windows, test))]
mod policy {
    use std::time::{Duration, SystemTime};

    pub struct StallDetector {
        threshold: Duration,
        reported: bool,
    }

    impl StallDetector {
        pub fn new(threshold: Duration) -> Self {
            Self {
                threshold,
                reported: false,
            }
        }

        pub fn observe(&mut self, heartbeat_age: Option<Duration>) -> Option<Duration> {
            let age = heartbeat_age?;
            if age < self.threshold {
                self.reported = false;
                return None;
            }
            if self.reported {
                return None;
            }
            self.reported = true;
            Some(age)
        }

        pub fn ignore_current_stall(&mut self) {
            self.reported = true;
        }
    }

    pub fn dumps_to_delete(
        mut dumps: Vec<(String, SystemTime)>,
        maximum_after_write: usize,
    ) -> Vec<String> {
        let keep = maximum_after_write.saturating_sub(1);
        if dumps.len() <= keep {
            return Vec::new();
        }
        dumps.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        let surplus = dumps.len() - keep;
        dumps.into_iter().take(surplus).map(|dump| dump.0).collect()
    }
}

#[cfg(any(windows, test))]
mod handoff {
    use std::hint::spin_loop;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};

    const NONE: u8 = 0;
    const RESERVED: u8 = 1;
    const CRASH: u8 = 2;
    const ABORT: u8 = 3;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        Crash,
        Abort,
    }

    impl Kind {
        fn code(self) -> u8 {
            match self {
                Self::Crash => CRASH,
                Self::Abort => ABORT,
            }
        }

        pub fn prefix(self) -> &'static str {
            match self {
                Self::Crash => "crash",
                Self::Abort => "abort",
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Request {
        pub kind: Kind,
        pub pointers: usize,
        pub thread_id: u32,
        pub code: u32,
    }

    /// Atomic-only handoff from a faulting thread or abort signal to the dump
    /// worker. `request` never allocates, formats, locks or performs I/O.
    pub struct Handoff {
        kind: AtomicU8,
        pointers: AtomicUsize,
        thread_id: AtomicU32,
        code: AtomicU32,
        done: AtomicBool,
    }

    impl Handoff {
        pub const fn new() -> Self {
            Self {
                kind: AtomicU8::new(NONE),
                pointers: AtomicUsize::new(0),
                thread_id: AtomicU32::new(0),
                code: AtomicU32::new(0),
                done: AtomicBool::new(false),
            }
        }

        pub fn request(&self, kind: Kind, pointers: usize, thread_id: u32, code: u32) -> bool {
            if self
                .kind
                .compare_exchange(NONE, RESERVED, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
            {
                return false;
            }
            self.pointers.store(pointers, Ordering::Relaxed);
            self.thread_id.store(thread_id, Ordering::Relaxed);
            self.code.store(code, Ordering::Relaxed);
            self.done.store(false, Ordering::Relaxed);
            self.kind.store(kind.code(), Ordering::Release);
            true
        }

        pub fn pending(&self) -> Option<Request> {
            let code = self.kind.load(Ordering::Acquire);
            let kind = match code {
                CRASH => Kind::Crash,
                ABORT => Kind::Abort,
                _ => return None,
            };
            Some(Request {
                kind,
                pointers: self.pointers.load(Ordering::Acquire),
                thread_id: self.thread_id.load(Ordering::Acquire),
                code: self.code.load(Ordering::Acquire),
            })
        }

        pub fn complete(&self) {
            self.done.store(true, Ordering::Release);
            self.kind.store(NONE, Ordering::Release);
        }

        pub fn is_complete(&self) -> bool {
            self.done.load(Ordering::Acquire)
        }

        pub fn wait(&self, spins: u32) -> bool {
            for _ in 0..spins {
                if self.is_complete() {
                    return true;
                }
                spin_loop();
            }
            self.is_complete()
        }
    }
}

#[cfg(any(windows, test))]
mod paths {
    use chrono::{DateTime, Utc};

    use crate::support::random_path_component;

    const DUMP_EXTENSION: &str = "dmp";

    /// Timestamp plus an unpredictable component. `create_new` still rejects a
    /// name planted by another process, even if the token were guessed.
    pub fn dump_file_name(prefix: &str, when: DateTime<Utc>) -> String {
        format!(
            "{prefix}-{}-{}.{DUMP_EXTENSION}",
            when.format("%Y%m%dT%H%M%SZ"),
            random_path_component()
        )
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant, SystemTime};

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE, MiniDumpWithThreadInfo,
        MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
    };
    use windows::core::BOOL;

    use super::handoff::{Handoff, Kind};
    use super::paths::dump_file_name;
    use super::policy::{StallDetector, dumps_to_delete};
    use crate::support::{ensure_private_dir, reject_symlink};

    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const HANG_THRESHOLD: Duration = Duration::from_secs(10);
    const MAX_DUMP_FILES: usize = 5;
    const WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);
    const DUMP_HANDOFF_WAIT_SPINS: u32 = 2_000_000;
    const DUMP_POLL_INTERVAL: Duration = Duration::from_millis(25);

    struct Paths {
        directory: PathBuf,
        crash_log: PathBuf,
    }

    struct DumpContext {
        pointers: usize,
        thread_id: u32,
    }

    static PATHS: OnceLock<Paths> = OnceLock::new();
    static STARTED: OnceLock<Instant> = OnceLock::new();
    static LAST_HEARTBEAT_MILLIS: AtomicU64 = AtomicU64::new(0);
    static WRITING: AtomicBool = AtomicBool::new(false);
    static HANDOFF: Handoff = Handoff::new();

    pub fn install(directory: PathBuf, crash_log: PathBuf) {
        if ensure_private_dir(&directory).is_err() || reject_symlink(&crash_log).is_err() {
            return;
        }
        if PATHS
            .set(Paths {
                directory,
                crash_log,
            })
            .is_err()
        {
            return;
        }
        STARTED.get_or_init(Instant::now);
        let _ = std::thread::Builder::new()
            .name("hang-watchdog".to_owned())
            .spawn(watch_for_hangs);
        let _ = std::thread::Builder::new()
            .name("crash-dump-worker".to_owned())
            .spawn(service_dump_requests);
        unsafe {
            SetUnhandledExceptionFilter(Some(exception_filter));
            libc::signal(libc::SIGABRT, abort_handler as *const () as usize);
        }
    }

    pub fn heartbeat() {
        let elapsed = STARTED.get_or_init(Instant::now).elapsed();
        LAST_HEARTBEAT_MILLIS.store(elapsed.as_millis() as u64 + 1, Ordering::Relaxed);
    }

    fn heartbeat_age() -> Option<Duration> {
        let stored = LAST_HEARTBEAT_MILLIS.load(Ordering::Relaxed);
        if stored == 0 {
            return None;
        }
        let elapsed = STARTED.get()?.elapsed();
        Some(elapsed.saturating_sub(Duration::from_millis(stored - 1)))
    }

    fn watch_for_hangs() {
        let mut detector = StallDetector::new(HANG_THRESHOLD);
        loop {
            let sleep_started = Instant::now();
            std::thread::sleep(WATCHDOG_INTERVAL);
            if sleep_started.elapsed() >= HANG_THRESHOLD {
                detector.ignore_current_stall();
            }
            let Some(stalled_for) = detector.observe(heartbeat_age()) else {
                continue;
            };
            if let Some(file_name) = write_dump("hang", None) {
                append_crash_log(&format!("hang {}s dump {file_name}", stalled_for.as_secs()));
            }
        }
    }

    /// Dumps are written only here: this thread may allocate, format and do I/O.
    fn service_dump_requests() {
        loop {
            if let Some(request) = HANDOFF.pending() {
                let prefix = request.kind.prefix();
                let context = (request.pointers != 0).then_some(DumpContext {
                    pointers: request.pointers,
                    thread_id: request.thread_id,
                });
                if let Some(file_name) = write_dump(prefix, context) {
                    let detail = if request.code == 0 {
                        format!("{prefix} dump {file_name}")
                    } else {
                        format!("{prefix} 0x{:08x} dump {file_name}", request.code)
                    };
                    append_crash_log(&detail);
                }
                HANDOFF.complete();
            }
            std::thread::sleep(DUMP_POLL_INTERVAL);
        }
    }

    /// Signal-safe: publish atomic state, then wait a bounded number of spins
    /// for the worker. No formatting, allocation, lock or I/O on this path.
    fn request_and_wait(kind: Kind, pointers: usize, thread_id: u32, code: u32) {
        if HANDOFF.request(kind, pointers, thread_id, code) {
            let _ = HANDOFF.wait(DUMP_HANDOFF_WAIT_SPINS);
        }
    }

    unsafe extern "system" fn exception_filter(pointers: *const EXCEPTION_POINTERS) -> i32 {
        let code = unsafe {
            pointers
                .as_ref()
                .and_then(|pointers| pointers.ExceptionRecord.as_ref())
                .map(|record| record.ExceptionCode.0 as u32)
                .unwrap_or_default()
        };
        let thread_id = unsafe { GetCurrentThreadId() };
        request_and_wait(Kind::Crash, pointers as usize, thread_id, code);
        EXCEPTION_CONTINUE_SEARCH
    }

    extern "C" fn abort_handler(_signal: i32) {
        let thread_id = unsafe { GetCurrentThreadId() };
        request_and_wait(Kind::Abort, 0, thread_id, 0);
        unsafe {
            libc::signal(libc::SIGABRT, libc::SIG_DFL);
            libc::abort();
        }
    }

    fn write_dump(prefix: &str, context: Option<DumpContext>) -> Option<String> {
        let paths = PATHS.get()?;
        if WRITING.swap(true, Ordering::SeqCst) {
            return None;
        }
        let file_name = write_dump_file(paths, prefix, context);
        WRITING.store(false, Ordering::SeqCst);
        file_name
    }

    fn write_dump_file(
        paths: &Paths,
        prefix: &str,
        context: Option<DumpContext>,
    ) -> Option<String> {
        if ensure_private_dir(&paths.directory).is_err() {
            return None;
        }
        prune_dumps(&paths.directory);
        let file_name = dump_file_name(prefix, chrono::Utc::now());
        let path = paths.directory.join(&file_name);
        if reject_symlink(&path).is_err() {
            return None;
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()?;
        let mut exception_information = context.map(|context| MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: context.thread_id,
            ExceptionPointers: context.pointers as *mut EXCEPTION_POINTERS,
            ClientPointers: BOOL(0),
        });
        let exception_parameter = exception_information
            .as_mut()
            .map(|information| information as *const MINIDUMP_EXCEPTION_INFORMATION);
        // No heap memory: it would hold access tokens and message text.
        let dump_type = MINIDUMP_TYPE(MiniDumpWithThreadInfo.0 | MiniDumpWithUnloadedModules.0);
        let written = unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                HANDLE(file.as_raw_handle()),
                dump_type,
                exception_parameter,
                None,
                None,
            )
        };
        written.ok().map(|_| file_name)
    }

    fn prune_dumps(directory: &Path) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let dumps = entries
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "dmp")
            })
            .map(|entry| {
                // symlink_metadata: pruning must never follow a planted link.
                let modified = std::fs::symlink_metadata(entry.path())
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (entry.file_name().to_string_lossy().into_owned(), modified)
            })
            .collect();
        for file_name in dumps_to_delete(dumps, MAX_DUMP_FILES) {
            let _ = fs::remove_file(directory.join(file_name));
        }
    }

    fn append_crash_log(detail: &str) {
        let Some(paths) = PATHS.get() else {
            return;
        };
        if reject_symlink(&paths.crash_log).is_err() {
            return;
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.crash_log)
        {
            let _ = writeln!(
                file,
                "=== {} build {} {detail}",
                chrono::Utc::now().to_rfc3339(),
                crate::version::build_version(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::handoff::{Handoff, Kind};
    use super::paths::dump_file_name;
    use super::policy::{StallDetector, dumps_to_delete};
    use crate::support::reject_symlink;

    fn seconds(value: u64) -> Option<Duration> {
        Some(Duration::from_secs(value))
    }

    #[test]
    fn no_report_before_first_heartbeat() {
        let mut detector = StallDetector::new(Duration::from_secs(10));
        assert_eq!(detector.observe(None), None);
    }

    #[test]
    fn reports_once_after_threshold() {
        let mut detector = StallDetector::new(Duration::from_secs(10));
        assert_eq!(detector.observe(seconds(9)), None);
        assert_eq!(detector.observe(seconds(10)), seconds(10));
        assert_eq!(detector.observe(seconds(11)), None);
        assert_eq!(detector.observe(seconds(30)), None);
    }

    #[test]
    fn rearms_after_heartbeats_resume() {
        let mut detector = StallDetector::new(Duration::from_secs(10));
        assert_eq!(detector.observe(seconds(12)), seconds(12));
        assert_eq!(detector.observe(seconds(1)), None);
        assert_eq!(detector.observe(seconds(15)), seconds(15));
    }

    #[test]
    fn a_stall_over_a_system_sleep_is_not_reported() {
        let mut detector = StallDetector::new(Duration::from_secs(10));
        detector.ignore_current_stall();
        assert_eq!(detector.observe(seconds(3600)), None);
        assert_eq!(detector.observe(seconds(1)), None);
        assert_eq!(detector.observe(seconds(15)), seconds(15));
    }

    fn dump(name: &str, age: u64) -> (String, SystemTime) {
        (
            name.to_owned(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(age),
        )
    }

    #[test]
    fn keeps_room_for_the_new_dump() {
        let dumps = vec![
            dump("e.dmp", 5),
            dump("a.dmp", 1),
            dump("c.dmp", 3),
            dump("b.dmp", 2),
            dump("d.dmp", 4),
        ];
        assert_eq!(dumps_to_delete(dumps, 5), vec!["a.dmp".to_owned()]);
    }

    #[test]
    fn deletes_nothing_below_the_limit() {
        let dumps = vec![dump("a.dmp", 1), dump("b.dmp", 2)];
        assert!(dumps_to_delete(dumps, 5).is_empty());
    }

    #[test]
    fn deletes_all_surplus_oldest_first() {
        let dumps = (1..=8)
            .map(|age| dump(&format!("{age}.dmp"), age))
            .collect();
        assert_eq!(
            dumps_to_delete(dumps, 5),
            vec!["1.dmp", "2.dmp", "3.dmp", "4.dmp"]
        );
    }

    #[test]
    fn dump_handoff_publishes_payload_and_completes() {
        let handoff = Handoff::new();
        assert!(handoff.pending().is_none());
        assert!(handoff.request(Kind::Crash, 0x1234, 42, 0xc0000005));
        let request = handoff.pending().unwrap();
        assert_eq!(request.kind, Kind::Crash);
        assert_eq!(request.pointers, 0x1234);
        assert_eq!(request.thread_id, 42);
        assert_eq!(request.code, 0xc0000005);
        assert!(!handoff.is_complete());
        assert!(!handoff.wait(0));
        handoff.complete();
        assert!(handoff.is_complete());
        assert!(handoff.pending().is_none());
        assert!(handoff.wait(1));
    }

    #[test]
    fn dump_handoff_rejects_a_second_request_while_pending() {
        let handoff = Handoff::new();
        assert!(handoff.request(Kind::Abort, 0, 7, 0));
        assert!(!handoff.request(Kind::Crash, 0x99, 8, 1));
        let request = handoff.pending().unwrap();
        assert_eq!(request.kind, Kind::Abort);
        assert_eq!(request.pointers, 0);
    }

    #[test]
    fn dump_names_are_random_and_keep_prefix() {
        let when = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let first = dump_file_name("crash", when);
        let second = dump_file_name("crash", when);
        assert_ne!(first, second);
        assert!(first.starts_with("crash-"));
        assert!(first.ends_with(".dmp"));
        assert!(first.contains("20231114"));
        assert_eq!(Kind::Crash.prefix(), "crash");
        assert_eq!(Kind::Abort.prefix(), "abort");
    }

    #[cfg(unix)]
    #[test]
    fn dump_paths_refuse_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target.dmp");
        let link = root.path().join("link.dmp");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(reject_symlink(&link).is_err());
        assert!(reject_symlink(&target).is_ok());
        assert!(reject_symlink(&root.path().join("missing.dmp")).is_ok());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }
}
