use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub const DIAGNOSTICS_FILE: &str = "diagnostics.log";
pub const DIAGNOSTICS_SETTING_KEY: &str = "diagnostics_enabled";
pub const ENV_DEBUG_LOG: &str = "RUSTY_TEAMS_DEBUG_LOG";

const MAX_BYTES: u64 = 1024 * 1024;
const RING_SLOTS: u32 = 2;
const MERGE_CAP_BYTES: u64 = 1024 * 1024;
const SLOW_UI_HANDLER: std::time::Duration = std::time::Duration::from_millis(30);

const REDACTED_IP: &str = "[ip]";
const REDACTED_ID: &str = "[id]";
const MIN_HEX_ID_BYTES: usize = 16;
const LONG_HEX_ID_BYTES: usize = 32;
const ID_KEY_MARKERS: [&str; 14] = [
    "tenant_id",
    "tenant-id",
    "conversation_id",
    "conversation-id",
    "user_id",
    "user-id",
    "account_id",
    "account-id",
    "chat_id",
    "chat-id",
    "room_id",
    "room-id",
    "call_id",
    "call-id",
];

static PATH: OnceLock<PathBuf> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

const OWN_CRATE_PREFIXES: [&str; 10] = [
    "app", "app_base", "state", "calling", "chatsvc", "session", "webview", "mcp", "agents",
    "teams",
];

const WEBRTC_KEPT_MARKERS: [&str; 6] = [
    "MFH264",
    "AdmProxy",
    "audio_device",
    "core_audio",
    "WebRtcVoiceEngine",
    "webrtc_voice_engine",
];
const WEBRTC_SKIPPED: [&str; 3] = ["audio_device_buffer", "requesting data", "will write"];
const ENDPOINT_DUMP_LINES: std::ops::Range<u32> = 4040..4170;

pub fn install(path: PathBuf) {
    let _ = PATH.set(path);
    if log::set_logger(&DiagnosticsLog).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
    crate::support::install_panic_diagnostics_hook();
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enable_from_override() -> bool {
    let on = std::env::args().any(|arg| arg == "--debug-log")
        || std::env::var(ENV_DEBUG_LOG)
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
    if on {
        set_enabled(true);
    }
    on
}

pub fn load_enabled_from_store(stored: Option<&str>) -> bool {
    let on = stored == Some("1");
    set_enabled(on || is_enabled());
    is_enabled()
}

pub fn directory() -> Option<PathBuf> {
    PATH.get().and_then(|path| path.parent()).map(PathBuf::from)
}

pub fn path() -> Option<PathBuf> {
    PATH.get().cloned()
}

pub fn record_if_slow(label: &str, elapsed: std::time::Duration) {
    if elapsed >= SLOW_UI_HANDLER {
        record(format!(
            "slow ui handler: {label} took {} ms",
            elapsed.as_millis()
        ));
    }
}

pub fn record(message: impl AsRef<str>) {
    let Some(path) = PATH.get() else {
        return;
    };
    write_line(path, is_enabled(), message.as_ref());
}

/// Single write boundary: while diagnostics are off nothing is printed or
/// persisted, and every line is redacted before it reaches stderr or the file.
fn write_line(path: &Path, enabled: bool, message: &str) {
    if !enabled {
        return;
    }
    let line = redact(message);
    eprintln!("{line}");
    if let Err(error) = append_rotated(
        path,
        MAX_BYTES,
        RING_SLOTS,
        &format!("{} {line}\n", line_prefix()),
    ) {
        eprintln!("diagnostics write failed: {error}");
    }
}

/// Mask addresses and identifiers at write time so no caller can persist a raw
/// remote payload, including WebRTC ICE lines from `libwebrtc`.
fn redact(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if let Some((value_start, value_end)) = named_id_value(bytes, index) {
            out.push_str(&text[index..value_start]);
            out.push_str(REDACTED_ID);
            index = value_end;
            continue;
        }
        if let Some(end) = uuid_end(bytes, index) {
            out.push_str(REDACTED_ID);
            index = end;
            continue;
        }
        if let Some(end) = ipv6_end(bytes, index) {
            out.push_str(REDACTED_IP);
            index = end;
            continue;
        }
        if let Some(end) = ipv4_end(bytes, index) {
            out.push_str(REDACTED_IP);
            index = end;
            continue;
        }
        if let Some(end) = long_hex_end(bytes, index) {
            out.push_str(REDACTED_ID);
            index = end;
            continue;
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        out.push(character);
        index += character.len_utf8();
    }
    out
}

fn named_id_value(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let marker = ID_KEY_MARKERS
        .iter()
        .find(|marker| bytes[index..].starts_with(marker.as_bytes()))?;
    let mut cursor = index + marker.len();
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
        cursor += 1;
    }
    if !matches!(bytes.get(cursor), Some(b':' | b'=')) {
        return None;
    }
    cursor += 1;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
        cursor += 1;
    }
    let value_start = cursor;
    while bytes.get(cursor).is_some_and(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'@' | b'+' | b'/' | b'=')
    }) {
        cursor += 1;
    }
    (cursor > value_start).then_some((value_start, cursor))
}

fn uuid_end(bytes: &[u8], index: usize) -> Option<usize> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut cursor = index;
    for (position, length) in GROUPS.iter().enumerate() {
        if position > 0 && bytes.get(cursor) != Some(&b'-') {
            return None;
        }
        if position > 0 {
            cursor += 1;
        }
        let end = cursor + length;
        if end > bytes.len() || !bytes[cursor..end].iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        cursor = end;
    }
    if bytes.get(cursor).is_some_and(u8::is_ascii_hexdigit) || bytes.get(cursor) == Some(&b'-') {
        return None;
    }
    Some(cursor)
}

fn ipv4_end(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && matches!(bytes[index - 1], b'0'..=b'9' | b'.') {
        return None;
    }
    let mut cursor = index;
    for octet in 0..4 {
        if octet > 0 && bytes.get(cursor) != Some(&b'.') {
            return None;
        }
        if octet > 0 {
            cursor += 1;
        }
        let start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) && cursor - start < 3 {
            cursor += 1;
        }
        if cursor == start {
            return None;
        }
        let value = bytes[start..cursor]
            .iter()
            .fold(0u16, |value, digit| value * 10 + u16::from(*digit - b'0'));
        if value > 255 {
            return None;
        }
    }
    if bytes.get(cursor).is_some_and(u8::is_ascii_digit) || bytes.get(cursor) == Some(&b'.') {
        return None;
    }
    Some(cursor)
}

fn ipv6_end(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && (bytes[index - 1].is_ascii_hexdigit() || bytes[index - 1] == b':') {
        return None;
    }
    let mut cursor = index;
    let mut groups = 0usize;
    let mut colons = 0usize;
    let mut double_colon = false;
    let mut distinctive = false;
    while let Some(byte) = bytes.get(cursor) {
        if byte.is_ascii_hexdigit() {
            let start = cursor;
            while bytes.get(cursor).is_some_and(u8::is_ascii_hexdigit) {
                cursor += 1;
            }
            groups += 1;
            if cursor - start != 2 || bytes[start..cursor].iter().any(u8::is_ascii_alphabetic) {
                distinctive = true;
            }
            continue;
        }
        if *byte == b':' {
            if bytes.get(cursor + 1) == Some(&b':') {
                double_colon = true;
            }
            colons += 1;
            cursor += 1;
            continue;
        }
        break;
    }
    if colons < 2 || groups < 2 || (!double_colon && !distinctive) {
        return None;
    }
    if bytes.get(cursor) == Some(&b':') {
        return None;
    }
    Some(cursor)
}

fn long_hex_end(bytes: &[u8], index: usize) -> Option<usize> {
    if !bytes.get(index).is_some_and(u8::is_ascii_hexdigit) {
        return None;
    }
    if index > 0 && bytes[index - 1].is_ascii_hexdigit() {
        return None;
    }
    let mut cursor = index;
    while bytes.get(cursor).is_some_and(u8::is_ascii_hexdigit) {
        cursor += 1;
    }
    let length = cursor - index;
    let has_letter = bytes[index..cursor].iter().any(u8::is_ascii_alphabetic);
    (length >= MIN_HEX_ID_BYTES && (has_letter || length >= LONG_HEX_ID_BYTES)).then_some(cursor)
}

/// Chronological merge of rotated slots then current (capped), for support packages.
pub fn read_merged(max_bytes: u64) -> String {
    let Some(path) = PATH.get() else {
        return String::new();
    };
    read_ring_merged(path, RING_SLOTS, max_bytes.min(MERGE_CAP_BYTES).max(1))
}

pub fn clear_logs() -> std::io::Result<()> {
    let Some(root) = directory() else {
        return Ok(());
    };
    for name in [
        DIAGNOSTICS_FILE,
        &format!("{DIAGNOSTICS_FILE}.1"),
        &format!("{DIAGNOSTICS_FILE}.2"),
        crate::crash_log::CRASH_LOG_FILE,
        &format!("{}.1", crate::crash_log::CRASH_LOG_FILE),
    ] {
        let path = root.join(name);
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
    }
    let dumps = root.join(crate::support::CALL_DUMPS_DIR);
    if dumps.is_dir() {
        for entry in fs::read_dir(&dumps)?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(path);
            } else {
                let _ = fs::remove_file(path);
            }
        }
    }
    Ok(())
}

pub(crate) fn append_rotated(
    path: &Path,
    max_bytes: u64,
    ring_slots: u32,
    line: &str,
) -> std::io::Result<()> {
    if path
        .metadata()
        .map(|meta| meta.len() >= max_bytes)
        .unwrap_or(false)
    {
        rotate_ring(path, ring_slots)?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

fn rotate_ring(path: &Path, ring_slots: u32) -> std::io::Result<()> {
    for slot in (1..ring_slots).rev() {
        let from = ring_path(path, slot);
        let to = ring_path(path, slot + 1);
        if from.exists() {
            let _ = fs::remove_file(&to);
            fs::rename(&from, &to)?;
        }
    }
    let first = ring_path(path, 1);
    let _ = fs::remove_file(&first);
    if path.exists() {
        fs::rename(path, &first)?;
    }
    Ok(())
}

fn ring_path(path: &Path, slot: u32) -> PathBuf {
    path.with_file_name(format!(
        "{}.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(DIAGNOSTICS_FILE),
        slot
    ))
}

pub(crate) fn read_ring_merged(path: &Path, slots: u32, max_bytes: u64) -> String {
    let mut parts = Vec::new();
    for slot in (1..=slots).rev() {
        let sibling = ring_path(path, slot);
        if let Ok(text) = fs::read_to_string(&sibling) {
            parts.push(text);
        }
    }
    if let Ok(text) = fs::read_to_string(path) {
        parts.push(text);
    }
    let merged = parts.join("");
    if merged.len() as u64 <= max_bytes {
        return merged;
    }
    let mut start = merged.len() - max_bytes as usize;
    while !merged.is_char_boundary(start) {
        start += 1;
    }
    merged[start..].to_owned()
}

fn line_prefix() -> String {
    format!(
        "{} {}",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
        crate::version::build_version()
    )
}

struct DiagnosticsLog;

impl log::Log for DiagnosticsLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        if !is_enabled() {
            return false;
        }
        let target = metadata.target();
        if target == "libwebrtc" {
            return true;
        }
        if is_own_crate(target) {
            return metadata.level() <= log::Level::Info;
        }
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, entry: &log::Record<'_>) {
        if !self.enabled(entry.metadata()) {
            return;
        }
        let target = entry.metadata().target();
        let text = entry.args().to_string();
        if target == "libwebrtc" {
            if keeps_webrtc_line(entry.metadata().level(), &text) {
                record(format!("webrtc: {text}"));
            }
            return;
        }
        let level = entry.metadata().level().as_str().to_ascii_lowercase();
        let short = target.split("::").next().unwrap_or(target);
        record(format!("{level} {short}: {text}"));
    }

    fn flush(&self) {}
}

fn is_own_crate(target: &str) -> bool {
    let root = target.split("::").next().unwrap_or(target);
    OWN_CRATE_PREFIXES
        .iter()
        .any(|prefix| root == *prefix || root.starts_with(&format!("{prefix}_")))
}

fn keeps_webrtc_line(level: log::Level, text: &str) -> bool {
    if level <= log::Level::Warn
        && (text.contains("peer_connection")
            || text.contains("PeerConnection")
            || text.contains("ICE")
            || text.contains("ice_"))
    {
        return true;
    }
    WEBRTC_KEPT_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
        && !WEBRTC_SKIPPED.iter().any(|skipped| text.contains(skipped))
        && !is_endpoint_dump(text)
}

fn is_endpoint_dump(text: &str) -> bool {
    text.split_once("audio_device_core_win.cc:")
        .and_then(|(_, rest)| rest.split_once(')'))
        .and_then(|(line, _)| line.parse::<u32>().ok())
        .is_some_and(|line| ENDPOINT_DUMP_LINES.contains(&line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static TEST_ENABLED_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn hardware_encoder_lines_are_kept() {
        assert!(keeps_webrtc_line(
            log::Level::Info,
            "(mf_h264_encoder_impl.cc:250): MFH264: initialized 868x488 @30fps"
        ));
    }

    #[test]
    fn audio_lines_are_kept_and_endpoint_dumps_and_noise_are_skipped() {
        assert!(keeps_webrtc_line(
            log::Level::Debug,
            "(audio_device_impl.cc:123): Init"
        ));
        assert!(keeps_webrtc_line(
            log::Level::Debug,
            "(webrtc_voice_engine.cc:10): WebRtcVoiceEngine::Init"
        ));
        assert!(!keeps_webrtc_line(
            log::Level::Debug,
            "(audio_device_core_win.cc:4090): endpoint 3"
        ));
        assert!(keeps_webrtc_line(
            log::Level::Debug,
            "(audio_device_core_win.cc:412): AvRevertMmThreadCharacteristics() is OK"
        ));
        assert!(!keeps_webrtc_line(
            log::Level::Debug,
            "(audio_device_buffer.cc:450): [REC : 10000msec, 48kHz] callbacks: 1000"
        ));
        assert!(!keeps_webrtc_line(
            log::Level::Debug,
            "(audio_device_pulse_linux.cc:2236): requesting data"
        ));
        assert!(!keeps_webrtc_line(
            log::Level::Debug,
            "(audio_device_pulse_linux.cc:2256): will write"
        ));
        assert!(!keeps_webrtc_line(
            log::Level::Debug,
            "(peer_connection.cc:55): something"
        ));
        assert!(keeps_webrtc_line(
            log::Level::Warn,
            "(peer_connection.cc:55): ICE failed"
        ));
    }

    #[test]
    fn enabled_gate_defaults_and_toggles() {
        let _guard = TEST_ENABLED_LOCK.lock().unwrap();
        set_enabled(false);
        assert!(!is_enabled());
        set_enabled(true);
        assert!(is_enabled());
        set_enabled(false);
        assert!(!is_enabled());
    }

    #[test]
    fn ring_rotation_preserves_previous_content() {
        let root = tempdir().unwrap();
        let path = root.path().join(DIAGNOSTICS_FILE);
        fs::write(&path, "a".repeat(MAX_BYTES as usize)).unwrap();
        append_rotated(&path, MAX_BYTES, RING_SLOTS, "fresh-line\n").unwrap();
        assert!(ring_path(&path, 1).exists());
        let current = fs::read_to_string(&path).unwrap();
        assert!(current.contains("fresh-line"));
        assert!(!current.contains(&"a".repeat(32)));
        let previous = fs::read_to_string(ring_path(&path, 1)).unwrap();
        assert!(previous.starts_with('a'));
    }

    #[test]
    fn own_crate_prefixes_match() {
        assert!(is_own_crate("app::linux_browser"));
        assert!(is_own_crate("state::notice"));
        assert!(is_own_crate("app_base::diagnostics"));
        assert!(!is_own_crate("hyper::client"));
    }

    #[test]
    fn webrtc_logging_is_gated_by_the_setting() {
        let _guard = TEST_ENABLED_LOCK.lock().unwrap();
        let metadata = log::Metadata::builder()
            .level(log::Level::Warn)
            .target("libwebrtc")
            .build();
        set_enabled(false);
        assert!(!DiagnosticsLog.enabled(&metadata));
        set_enabled(true);
        assert!(DiagnosticsLog.enabled(&metadata));
        set_enabled(false);
    }

    #[test]
    fn redaction_masks_addresses_and_identifiers() {
        let line = redact(
            "ICE 10.0.0.1:3478 fe80::1 tenant_id=abc-123 conversation-id: 00000000-0000-0000-0000-000000000001 token=0123456789abcdef0123456789abcdef",
        );
        assert!(line.contains(REDACTED_IP));
        assert!(line.contains(REDACTED_ID));
        assert!(!line.contains("10.0.0.1"));
        assert!(!line.contains("fe80::1"));
        assert!(!line.contains("abc-123"));
        assert!(!line.contains("00000000-0000-0000-0000-000000000001"));
        assert!(!line.contains("0123456789abcdef0123456789abcdef"));
        assert_eq!(redact(&line), line);
        assert_eq!(redact("2026-11-24T12:34:56Z"), "2026-11-24T12:34:56Z");
    }

    #[test]
    fn disabled_lines_are_not_written() {
        let root = tempdir().unwrap();
        let path = root.path().join(DIAGNOSTICS_FILE);
        let ice = "webrtc: ICE failed for 192.168.1.10 tenant_id=abc123";
        write_line(&path, false, ice);
        assert!(!path.exists());
        write_line(&path, true, ice);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("webrtc: ICE failed"));
        assert!(text.contains(REDACTED_IP));
        assert!(text.contains(REDACTED_ID));
        assert!(!text.contains("192.168.1.10"));
        assert!(!text.contains("abc123"));
    }
}
