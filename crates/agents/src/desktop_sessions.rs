use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopSession {
    pub session_id: String,
    pub working_dir: String,
    pub title: String,
    pub last_activity_millis: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum DesktopSessionsError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopConfig {
    #[serde(rename = "lastKnownAccountUuid")]
    account_uuid: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopSessionFile {
    #[serde(rename = "cliSessionId")]
    cli_session_id: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    #[serde(rename = "isArchived", default)]
    archived: bool,
    #[serde(rename = "lastActivityAt", default)]
    last_activity_millis: i64,
}

fn is_plain_account_uuid(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn io_error(path: &Path, source: std::io::Error) -> DesktopSessionsError {
    DesktopSessionsError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn json_error(path: &Path, source: serde_json::Error) -> DesktopSessionsError {
    DesktopSessionsError::Json {
        path: path.to_path_buf(),
        source,
    }
}

fn read_optional_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, DesktopSessionsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(path, source)),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|source| json_error(path, source))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, DesktopSessionsError> {
    let text = std::fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    serde_json::from_str(&text).map_err(|source| json_error(path, source))
}

fn optional_dir(path: &Path) -> Result<Option<std::fs::ReadDir>, DesktopSessionsError> {
    match std::fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
    }
}

pub fn desktop_sessions(desktop_dir: &Path) -> Result<Vec<DesktopSession>, DesktopSessionsError> {
    let config_path = desktop_dir.join("config.json");
    let config = read_optional_json::<DesktopConfig>(&config_path)?;
    let Some(account_uuid) = config
        .and_then(|config| config.account_uuid)
        .filter(|uuid| is_plain_account_uuid(uuid))
    else {
        return Ok(Vec::new());
    };
    let sessions_dir = desktop_dir.join("claude-code-sessions").join(account_uuid);
    let Some(organizations) = optional_dir(&sessions_dir)? else {
        return Ok(Vec::new());
    };
    let mut sessions = Vec::new();
    for organization in organizations {
        let organization = organization.map_err(|source| io_error(&sessions_dir, source))?;
        let organization_path = organization.path();
        let Some(files) = optional_dir(&organization_path)? else {
            continue;
        };
        for entry in files {
            let entry = entry.map_err(|source| io_error(&organization_path, source))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("local_") && name.ends_with(".json")) {
                continue;
            }
            let file: DesktopSessionFile = read_json(&entry.path())?;
            if file.archived {
                continue;
            }
            let Some(session_id) = file.cli_session_id.filter(|id| !id.is_empty()) else {
                continue;
            };
            sessions.push(DesktopSession {
                session_id,
                working_dir: file.cwd.unwrap_or_default(),
                title: file.title.unwrap_or_default(),
                last_activity_millis: file.last_activity_millis,
            });
        }
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn only_active_sessions_of_the_signed_in_account_with_a_cli_id_are_listed() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join("config.json"),
            r#"{"lastKnownAccountUuid":"acc-1"}"#,
        );
        let sessions = root.path().join("claude-code-sessions");
        write(
            &sessions.join("acc-1/org-1/local_a.json"),
            r#"{"cliSessionId":"cli-a","cwd":"/srv/app","title":"Fix build","isArchived":false,"lastActivityAt":1700000000000}"#,
        );
        write(
            &sessions.join("acc-1/org-2/local_b.json"),
            r#"{"cliSessionId":"cli-b","cwd":"/srv/b","title":"Archived","isArchived":true,"lastActivityAt":1}"#,
        );
        write(
            &sessions.join("acc-1/org-1/local_c.json"),
            r#"{"cwd":"/srv/c","title":"No cli id","isArchived":false}"#,
        );
        write(
            &sessions.join("acc-2/org-1/local_d.json"),
            r#"{"cliSessionId":"cli-d","cwd":"/srv/d","title":"Other account","isArchived":false}"#,
        );
        write(&sessions.join("acc-1/org-1/notes.json"), "{}");
        assert_eq!(
            desktop_sessions(root.path()).unwrap(),
            [DesktopSession {
                session_id: "cli-a".to_owned(),
                working_dir: "/srv/app".to_owned(),
                title: "Fix build".to_owned(),
                last_activity_millis: 1_700_000_000_000,
            }]
        );
    }

    #[test]
    fn a_missing_config_or_an_unsafe_account_id_lists_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(desktop_sessions(root.path()).unwrap().is_empty());
        write(
            &root.path().join("config.json"),
            r#"{"lastKnownAccountUuid":"../x"}"#,
        );
        assert!(desktop_sessions(root.path()).unwrap().is_empty());
    }

    #[test]
    fn unknown_fields_in_desktop_files_surface_an_error() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.json");
        write(&config, r#"{"lastKnownAccountUuid":"acc-1","extra":1}"#);
        assert!(desktop_sessions(root.path()).is_err());
        write(&config, r#"{"lastKnownAccountUuid":"acc-1"}"#);
        write(
            &root
                .path()
                .join("claude-code-sessions/acc-1/org/local_a.json"),
            r#"{"cliSessionId":"cli-a","unknown":true}"#,
        );
        assert!(desktop_sessions(root.path()).is_err());
    }

    #[test]
    fn malformed_session_files_surface_an_error() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join("config.json"),
            r#"{"lastKnownAccountUuid":"acc-1"}"#,
        );
        write(
            &root
                .path()
                .join("claude-code-sessions/acc-1/org/local_a.json"),
            "not json",
        );
        assert!(desktop_sessions(root.path()).is_err());
    }
}
