//! Redaction shared by WebView logs and diagnostic scripts.

const REDACTED: &str = "{id}";
const MIN_HEX_ID: usize = 16;
const MIN_OPAQUE_ID: usize = 32;
const MIN_CONVERSATION_ID: usize = 20;
const MAX_CDP_MESSAGE: usize = 200;

pub(crate) const ENV_OS_SSO: &str = "RUSTY_TEAMS_ALLOW_OS_SSO";
pub(crate) const ENV_CALL_SHADOW: &str = "RUSTY_TEAMS_CALL_SHADOW";

/// Explicit opt-in only: an unset or unrecognised value keeps the protection off.
pub(crate) fn env_flag(value: Option<&str>) -> bool {
    matches!(
        value,
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

fn is_id_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '%' | ':' | '@' | '.' | '-')
}

fn is_hex_id(token: &str) -> bool {
    token.len() >= MIN_HEX_ID
        && token
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
}

/// Replaces tenant, conversation and other opaque identifiers with `{id}`.
pub(crate) fn redact_identifiers(input: &str) -> String {
    let mut redacted = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(offset) = rest.find(is_id_char) {
        redacted.push_str(&rest[..offset]);
        rest = &rest[offset..];
        let end = rest
            .find(|character: char| !is_id_char(character))
            .unwrap_or(rest.len());
        let token = &rest[..end];
        let opaque = token.starts_with("19:")
            && token.len() >= MIN_CONVERSATION_ID
            && token
                .chars()
                .any(|character| !character.is_ascii_hexdigit() && character != '-');
        if is_hex_id(token) || token.len() >= MIN_OPAQUE_ID || opaque {
            redacted.push_str(REDACTED);
        } else {
            redacted.push_str(token);
        }
        rest = &rest[end..];
    }
    redacted.push_str(rest);
    redacted
}

/// Keeps protocol error messages bounded and free of identifiers.
pub(crate) fn cdp_message(message: &str) -> String {
    let redacted = redact_identifiers(message);
    if redacted.chars().count() <= MAX_CDP_MESSAGE {
        return redacted;
    }
    let mut shortened: String = redacted.chars().take(MAX_CDP_MESSAGE).collect();
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_redacted_without_losing_the_path() {
        let input = "teams.cloud.microsoft/api/chats/8f3a2c10-6b4d-4f1e-9a02-7c1d5e8b0f44/19:abcDEF1234567890@thread.v2";
        let redacted = redact_identifiers(input);
        assert!(!redacted.contains("8f3a2c10-6b4d-4f1e-9a02-7c1d5e8b0f44"));
        assert!(!redacted.contains("19:abcDEF1234567890@thread.v2"));
        assert_eq!(redacted, "teams.cloud.microsoft/api/chats/{id}/{id}");
    }

    #[test]
    fn ordinary_labels_and_ascii_are_untouched() {
        assert_eq!(
            redact_identifiers("teams.cloud.microsoft/api/csa"),
            "teams.cloud.microsoft/api/csa"
        );
        assert_eq!(
            redact_identifiers("The parameter is incorrect. (0x80070057)"),
            "The parameter is incorrect. (0x80070057)"
        );
        assert_eq!(redact_identifiers("callShadow"), "callShadow");
    }

    #[test]
    fn cdp_messages_are_redacted_and_bounded() {
        let message = format!(
            "failed for tenant 8f3a2c10-6b4d-4f1e-9a02-7c1d5e8b0f44: {}",
            "plain words ".repeat(MAX_CDP_MESSAGE)
        );
        let shortened = cdp_message(&message);
        assert!(shortened.chars().count() <= MAX_CDP_MESSAGE + 1);
        assert!(shortened.ends_with('…'));
        assert!(!shortened.contains("8f3a2c10-6b4d-4f1e-9a02-7c1d5e8b0f44"));
        assert_eq!(cdp_message("short message"), "short message");
    }

    #[test]
    fn opt_in_flags_need_an_explicit_true() {
        assert!(env_flag(Some("1")));
        assert!(env_flag(Some("true")));
        assert!(env_flag(Some("YES")));
        assert!(!env_flag(None));
        assert!(!env_flag(Some("0")));
        assert!(!env_flag(Some("false")));
    }
}
