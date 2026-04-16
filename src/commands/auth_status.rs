use crate::credentials::{self, UserIdentity};
use crate::error::Result;
use crate::output::PlainText;
use colored::Colorize;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BotSummary {
    pub id: i64,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub data_dir: String,
    pub session_stored: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserIdentity>,
    pub bots: Vec<BotSummary>,
}

pub fn build_auth_status(data_dir: &Path) -> Result<AuthStatus> {
    let creds = credentials::load_credentials_file(data_dir).ok();
    let session_stored = session_dir_has_entries(data_dir);

    let (api_id, user, bots) = match creds.as_ref() {
        Some(creds) => {
            let bots = creds
                .bots
                .iter()
                .map(|b| BotSummary {
                    id: b.id,
                    username: b.username.clone(),
                })
                .collect();
            let api_id = if creds.api_id > 0 && !creds.api_hash.is_empty() {
                Some(creds.api_id)
            } else {
                None
            };
            (api_id, creds.user.clone(), bots)
        }
        None => (None, None, Vec::new()),
    };

    let authenticated = session_stored && api_id.is_some();

    Ok(AuthStatus {
        authenticated,
        data_dir: data_dir.display().to_string(),
        session_stored,
        api_id,
        user,
        bots,
    })
}

fn session_dir_has_entries(data_dir: &Path) -> bool {
    dir_has_entries(&data_dir.join("db")) || dir_has_entries(&data_dir.join("files"))
}

fn dir_has_entries(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => false,
    }
}

fn yes_no(b: bool) -> String {
    if b {
        "yes".green().to_string()
    } else {
        "no".red().to_string()
    }
}

impl PlainText for AuthStatus {
    fn to_plain_text(&self) -> String {
        let label_width = 16;
        let mut lines = Vec::new();

        lines.push(format!(
            "{:<label_width$} {}",
            "Authenticated:",
            yes_no(self.authenticated),
            label_width = label_width
        ));
        lines.push(format!(
            "{:<label_width$} {}",
            "Data directory:",
            self.data_dir,
            label_width = label_width
        ));
        lines.push(format!(
            "{:<label_width$} {}",
            "Session stored:",
            yes_no(self.session_stored),
            label_width = label_width
        ));

        match self.api_id {
            Some(id) => lines.push(format!(
                "{:<label_width$} {}",
                "API ID:",
                id,
                label_width = label_width
            )),
            None => lines.push(format!(
                "{:<label_width$} {}",
                "API ID:",
                "(not set)".dimmed(),
                label_width = label_width
            )),
        }

        if let Some(user) = &self.user {
            let u = match user.username.as_deref() {
                Some(name) => format!("@{} ({})", name, user.id),
                None => format!("({})", user.id),
            };
            lines.push(format!(
                "{:<label_width$} {}",
                "User:",
                u,
                label_width = label_width
            ));
        }

        lines.push(format!("Bots ({}):", self.bots.len()).bold().to_string());
        if self.bots.is_empty() {
            lines.push(format!("  {}", "(none)".dimmed()));
        } else {
            for bot in &self.bots {
                lines.push(format!("  @{}  ({})", bot.username, bot.id));
            }
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{
        ApiCredentials, BotEntry, CredentialsFile, KnownContact, save_credentials,
        save_credentials_file,
    };

    fn sample_creds() -> CredentialsFile {
        CredentialsFile {
            api_id: 12345,
            api_hash: "deadbeef".to_string(),
            user: Some(UserIdentity {
                id: 9001,
                username: Some("me".to_string()),
            }),
            bots: vec![BotEntry {
                id: 100,
                username: "kiramarbot".to_string(),
                token: "100:SECRET".to_string(),
            }],
            known_contacts: vec![KnownContact {
                id: 200,
                username: "friend".to_string(),
            }],
        }
    }

    #[test]
    fn status_reports_not_authenticated_when_no_files() {
        let tempdir = tempfile::tempdir().unwrap();
        let status = build_auth_status(tempdir.path()).unwrap();
        assert!(!status.authenticated);
        assert!(!status.session_stored);
        assert_eq!(status.api_id, None);
        assert!(status.bots.is_empty());
    }

    #[test]
    fn status_reports_authed_with_session_and_credentials() {
        let tempdir = tempfile::tempdir().unwrap();
        let data_dir = tempdir.path();
        std::fs::create_dir_all(data_dir.join("db")).unwrap();
        std::fs::write(data_dir.join("db").join("marker"), b"").unwrap();
        save_credentials_file(&sample_creds(), data_dir).unwrap();

        let status = build_auth_status(data_dir).unwrap();
        assert!(status.authenticated);
        assert!(status.session_stored);
        assert_eq!(status.api_id, Some(12345));
        assert_eq!(status.bots.len(), 1);
        assert_eq!(status.bots[0].username, "kiramarbot");
        assert_eq!(status.bots[0].id, 100);
        assert_eq!(
            status.user.as_ref().and_then(|u| u.username.clone()),
            Some("me".to_string())
        );
    }

    #[test]
    fn status_not_authed_when_session_missing() {
        let tempdir = tempfile::tempdir().unwrap();
        save_credentials_file(&sample_creds(), tempdir.path()).unwrap();
        let status = build_auth_status(tempdir.path()).unwrap();
        assert!(!status.authenticated);
        assert!(!status.session_stored);
        assert_eq!(status.api_id, Some(12345));
    }

    #[test]
    fn status_not_authed_when_api_creds_missing() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tempdir.path().join("db")).unwrap();
        std::fs::write(tempdir.path().join("db").join("x"), b"").unwrap();
        // credentials file has empty api_hash → treated as not set
        let creds = CredentialsFile {
            api_id: 12345,
            api_hash: String::new(),
            user: None,
            bots: Vec::new(),
            known_contacts: Vec::new(),
        };
        save_credentials_file(&creds, tempdir.path()).unwrap();

        let status = build_auth_status(tempdir.path()).unwrap();
        assert!(!status.authenticated);
        assert!(status.session_stored);
        assert_eq!(status.api_id, None);
    }

    #[test]
    fn status_does_not_leak_bot_tokens_in_json() {
        let tempdir = tempfile::tempdir().unwrap();
        save_credentials_file(&sample_creds(), tempdir.path()).unwrap();
        let status = build_auth_status(tempdir.path()).unwrap();
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("SECRET"));
        assert!(!json.contains("token"));
    }

    #[test]
    fn status_plain_text_contains_expected_sections() {
        let status = AuthStatus {
            authenticated: true,
            data_dir: "/tmp/tg".to_string(),
            session_stored: true,
            api_id: Some(12345),
            user: Some(UserIdentity {
                id: 9001,
                username: Some("me".to_string()),
            }),
            bots: vec![BotSummary {
                id: 100,
                username: "kiramarbot".to_string(),
            }],
        };
        let text = status.to_plain_text();
        assert!(text.contains("Authenticated:"));
        assert!(text.contains("Data directory:"));
        assert!(text.contains("/tmp/tg"));
        assert!(text.contains("API ID:"));
        assert!(text.contains("12345"));
        assert!(text.contains("@me"));
        assert!(text.contains("9001"));
        assert!(text.contains("Bots (1)"));
        assert!(text.contains("@kiramarbot"));
        assert!(text.contains("100"));
        assert!(!text.contains("Known contacts"));
    }

    #[test]
    fn status_plain_text_no_bots() {
        let status = AuthStatus {
            authenticated: false,
            data_dir: "/tmp/tg".to_string(),
            session_stored: false,
            api_id: None,
            user: None,
            bots: Vec::new(),
        };
        let text = status.to_plain_text();
        assert!(text.contains("Bots (0)"));
        assert!(text.contains("(none)"));
        assert!(text.contains("(not set)"));
    }

    #[test]
    fn save_then_status_roundtrips_api_id() {
        let tempdir = tempfile::tempdir().unwrap();
        save_credentials(
            &ApiCredentials {
                api_id: 42,
                api_hash: "h".to_string(),
            },
            tempdir.path(),
        )
        .unwrap();
        std::fs::create_dir_all(tempdir.path().join("db")).unwrap();
        std::fs::write(tempdir.path().join("db").join("x"), b"").unwrap();
        let status = build_auth_status(tempdir.path()).unwrap();
        assert_eq!(status.api_id, Some(42));
        assert!(status.authenticated);
    }
}
