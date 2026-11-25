use serde_json::Value;

/// Microsoft code for a silent token refresh that needs interactive MFA.
pub const INTERACTIVE_MFA_CODE: &str = "AADSTS50076";

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no browser on {endpoint}: {reason}")]
    NoBrowser { endpoint: String, reason: String },
    #[error("no open Teams or Outlook tab in the browser")]
    NoAppTab,
    #[error("Microsoft 365 login required{}", login_detail_suffix(.detail))]
    LoginRequired { detail: Option<String> },
    #[error("no fresh token for {scope}{detail}")]
    NoFreshToken { scope: String, detail: String },
    #[error("HTTP {status}: {message} ({url})")]
    Api {
        status: u16,
        url: String,
        message: String,
        body: Box<Value>,
    },
    #[error("browser protocol error: {0}")]
    Cdp(String),
    #[error("tab connection lost during a write ({0}); check whether it was sent before retrying")]
    WriteInterrupted(String),
}

fn login_detail_suffix(detail: &Option<String>) -> String {
    detail
        .as_deref()
        .map(|text| format!(": {text}"))
        .unwrap_or_default()
}

impl Error {
    pub fn api(status: u16, url: &str, body: Value) -> Self {
        let message = match body.pointer("/error/message").and_then(Value::as_str) {
            Some(text) => text.to_owned(),
            None => body.to_string().chars().take(300).collect(),
        };
        Error::Api {
            status,
            url: url.to_owned(),
            message,
            body: Box::new(body),
        }
    }

    /// The page's `refreshError` text, whichever login error variant carried it.
    pub fn login_detail(&self) -> Option<&str> {
        match self {
            Error::LoginRequired { detail } => detail.as_deref(),
            Error::NoFreshToken { detail, .. } => Some(detail),
            _ => None,
        }
    }

    /// Microsoft asked for interactive MFA while refreshing a token.
    pub fn requires_interactive_mfa(&self) -> bool {
        self.login_detail()
            .is_some_and(|detail| detail.contains(INTERACTIVE_MFA_CODE))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn api_error_uses_graph_message() {
        let error = Error::api(
            403,
            "https://graph.microsoft.com/v1.0/me",
            json!({"error": {"message": "Forbidden"}}),
        );
        assert_eq!(
            error.to_string(),
            "HTTP 403: Forbidden (https://graph.microsoft.com/v1.0/me)"
        );
    }

    #[test]
    fn api_error_truncates_plain_bodies() {
        let Error::Api { message, .. } = Error::api(0, "u", json!("x".repeat(1000))) else {
            unreachable!()
        };
        assert_eq!(message.chars().count(), 300);
    }

    #[test]
    fn login_errors_keep_the_refresh_detail() {
        let detail = format!("invalid_grant: {INTERACTIVE_MFA_CODE}: finish MFA in the browser");
        let error = Error::NoFreshToken {
            scope: "scope".into(),
            detail: detail.clone(),
        };
        assert_eq!(error.login_detail(), Some(detail.as_str()));
        assert!(error.requires_interactive_mfa());

        let error = Error::LoginRequired {
            detail: Some(detail.clone()),
        };
        assert_eq!(error.login_detail(), Some(detail.as_str()));
        assert!(error.requires_interactive_mfa());
        assert!(error.to_string().contains(INTERACTIVE_MFA_CODE));
    }

    #[test]
    fn other_login_failures_are_not_an_mfa_challenge() {
        let error = Error::LoginRequired {
            detail: Some("invalid_grant".into()),
        };
        assert!(!error.requires_interactive_mfa());

        let error = Error::LoginRequired { detail: None };
        assert_eq!(error.login_detail(), None);
        assert!(!error.requires_interactive_mfa());
        assert_eq!(error.to_string(), "Microsoft 365 login required");
    }
}
