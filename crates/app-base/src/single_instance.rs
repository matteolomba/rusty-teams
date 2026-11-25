use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

static ACTIVATION: AtomicBool = AtomicBool::new(false);
static COMMANDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

const RETRY_INTERVAL: Duration = Duration::from_millis(200);
const RETRY_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_QUEUED_COMMANDS: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    Primary,
    Forwarded,
}

pub fn take_activation() -> bool {
    ACTIVATION.swap(false, Ordering::AcqRel)
}

fn commands() -> MutexGuard<'static, Vec<String>> {
    COMMANDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Cuts a forwarded command at the first newline or NUL so a second verb cannot
/// be smuggled after a user command.
fn bounded_command(command: &str) -> &str {
    let end = command
        .bytes()
        .position(|byte| matches!(byte, b'\n' | b'\r' | 0))
        .unwrap_or(command.len());
    &command[..end]
}

fn first_command(message: &[u8]) -> &[u8] {
    let end = message
        .iter()
        .position(|byte| matches!(byte, b'\n' | b'\r' | 0))
        .unwrap_or(message.len());
    &message[..end]
}

pub fn queue_command(command: &str) {
    let command = bounded_command(command).trim();
    if command.is_empty() {
        return;
    }
    let mut commands = commands();
    if commands.len() >= MAX_QUEUED_COMMANDS {
        commands.remove(0);
    }
    commands.push(command.to_owned());
}

pub fn take_commands() -> Vec<String> {
    std::mem::take(&mut *commands())
}

fn received(message: &[u8]) {
    let message = &message[..message.len().min(MAX_MESSAGE_BYTES)];
    queue_command(String::from_utf8_lossy(first_command(message)).trim());
    ACTIVATION.store(true, Ordering::Release);
}

/// `command` travels to the running instance, which queues it for `take_commands`.
pub fn claim(name: &str, wait_for_previous: bool, command: &str) -> Claim {
    let command = bounded_command(command);
    let started = Instant::now();
    loop {
        if platform::try_become_primary(name) {
            queue_command(command);
            return Claim::Primary;
        }
        if !wait_for_previous {
            platform::notify_primary(name, command);
            return Claim::Forwarded;
        }
        if started.elapsed() >= RETRY_TIMEOUT {
            return Claim::Primary;
        }
        std::thread::sleep(RETRY_INTERVAL);
    }
}

fn user_name() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default()
}

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::mem::ManuallyDrop;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::time::Duration;

    use windows::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError,
        HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_INBOUND,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    use windows::core::PCWSTR;

    use super::{MAX_MESSAGE_BYTES, bounded_command, received, user_name};

    const CONNECT_ATTEMPTS: u32 = 20;
    const CONNECT_INTERVAL: Duration = Duration::from_millis(100);

    fn pipe_path(name: &str) -> String {
        format!(r"\\.\pipe\{name}-{}", user_name())
    }

    pub fn try_become_primary(name: &str) -> bool {
        let wide_path: Vec<u16> = pipe_path(name)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide_path.as_ptr()),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                0,
                0,
                None,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = unsafe { GetLastError() };
            return error != ERROR_ACCESS_DENIED && error != ERROR_PIPE_BUSY;
        }
        let raw_handle = handle.0 as usize;
        std::thread::spawn(move || {
            let handle = HANDLE(raw_handle as *mut _);
            loop {
                let connected = unsafe { ConnectNamedPipe(handle, None) }.is_ok()
                    || matches!(
                        unsafe { GetLastError() },
                        ERROR_PIPE_CONNECTED | ERROR_NO_DATA
                    );
                if connected {
                    let mut pipe = ManuallyDrop::new(unsafe { File::from_raw_handle(handle.0) });
                    let mut message = Vec::new();
                    let _ = (&mut *pipe)
                        .take(MAX_MESSAGE_BYTES as u64)
                        .read_to_end(&mut message);
                    received(&message);
                } else {
                    std::thread::sleep(CONNECT_INTERVAL);
                }
                let _ = unsafe { DisconnectNamedPipe(handle) };
            }
        });
        true
    }

    pub fn notify_primary(name: &str, command: &str) {
        let path = pipe_path(name);
        let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        for _ in 0..CONNECT_ATTEMPTS {
            if let Ok(mut pipe) = OpenOptions::new().write(true).open(&path) {
                let _ = pipe.write_all(bounded_command(command).as_bytes());
                let _ = unsafe { FlushFileBuffers(HANDLE(pipe.as_raw_handle())) };
                return;
            }
            std::thread::sleep(CONNECT_INTERVAL);
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::io::{ErrorKind, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{MAX_MESSAGE_BYTES, bounded_command, received, user_name};

    const READ_TIMEOUT: Duration = Duration::from_secs(1);

    pub(super) fn socket_path(name: &str) -> PathBuf {
        let directory = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        directory.join(format!("{name}-{}.sock", user_name()))
    }

    enum Bound {
        Listening(UnixListener),
        Taken,
        Unavailable,
    }

    fn bind(path: &PathBuf) -> Bound {
        match UnixListener::bind(path) {
            Ok(listener) => Bound::Listening(listener),
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                if UnixStream::connect(path).is_ok() {
                    return Bound::Taken;
                }
                let _ = std::fs::remove_file(path);
                UnixListener::bind(path).map_or(Bound::Unavailable, Bound::Listening)
            }
            Err(_) => Bound::Unavailable,
        }
    }

    pub fn try_become_primary(name: &str) -> bool {
        let listener = match bind(&socket_path(name)) {
            Bound::Listening(listener) => listener,
            Bound::Taken => return false,
            Bound::Unavailable => return true,
        };
        std::thread::spawn(move || {
            for mut connection in listener.incoming().flatten() {
                let _ = connection.set_read_timeout(Some(READ_TIMEOUT));
                let mut message = Vec::new();
                let _ = connection
                    .take(MAX_MESSAGE_BYTES as u64)
                    .read_to_end(&mut message);
                received(&message);
            }
        });
        true
    }

    pub fn notify_primary(name: &str, command: &str) {
        if let Ok(mut stream) = UnixStream::connect(socket_path(name)) {
            let _ = stream.write_all(bounded_command(command).as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_lock() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn reset() {
        take_commands();
        take_activation();
    }
    #[test]
    fn second_claim_forwards_and_stale_socket_is_reclaimed() {
        let _guard = test_lock();
        reset();
        let name = format!("rusty-teams-test-{}", std::process::id());
        assert_eq!(
            claim(&name, false, "join https://example/meet"),
            Claim::Primary
        );
        assert_eq!(take_commands(), ["join https://example/meet"]);
        assert_eq!(claim(&name, false, "hang-up"), Claim::Forwarded);
        let started = Instant::now();
        let mut commands = Vec::new();
        while commands.is_empty() {
            assert!(started.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(10));
            commands = take_commands();
        }
        assert_eq!(commands, ["hang-up"]);
        assert!(take_activation());

        let stale_name = format!("{name}-stale");
        #[cfg(unix)]
        {
            let path = platform::socket_path(&stale_name);
            drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
            assert!(path.exists());
        }
        assert_eq!(claim(&stale_name, false, ""), Claim::Primary);
        assert_eq!(
            claim("missing-directory/rusty-teams", false, ""),
            Claim::Primary
        );
        assert!(take_commands().is_empty());

        #[cfg(unix)]
        for leftover in [&name, &stale_name] {
            let _ = std::fs::remove_file(platform::socket_path(leftover));
        }
    }

    #[test]
    fn forwarded_commands_stop_at_newline_or_nul() {
        let _guard = test_lock();
        reset();
        assert_eq!(bounded_command("stats C:\\temp\njoin x"), "stats C:\\temp");
        assert_eq!(bounded_command("join x\0y"), "join x");
        received(b"join https://example/meet\nstats C:\\temp");
        assert_eq!(take_commands(), ["join https://example/meet"]);
        assert!(take_activation());

        reset();
        received(b"hang-up\0leftovers");
        assert_eq!(take_commands(), ["hang-up"]);
        assert!(take_activation());
    }

    #[test]
    fn oversized_pipe_messages_are_truncated_to_four_kib() {
        let _guard = test_lock();
        reset();
        let message = vec![b'x'; MAX_MESSAGE_BYTES + 64];
        received(&message);
        let commands = take_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].len(), MAX_MESSAGE_BYTES);
        assert!(take_activation());
    }

    #[test]
    fn the_command_queue_drops_the_oldest_entry_when_full() {
        let _guard = test_lock();
        reset();
        for index in 0..MAX_QUEUED_COMMANDS + 5 {
            queue_command(&format!("command-{index}"));
        }
        let commands = take_commands();
        assert_eq!(commands.len(), MAX_QUEUED_COMMANDS);
        assert_eq!(commands.first().unwrap(), "command-5");
        assert_eq!(
            commands.last().unwrap(),
            &format!("command-{}", MAX_QUEUED_COMMANDS + 4)
        );
    }
}
