use crate::error::{Result, TgError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CREDENTIALS_FILE: &str = "credentials.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiCredentials {
    pub api_id: i32,
    pub api_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserIdentity {
    pub id: i64,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotEntry {
    pub id: i64,
    pub username: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownContact {
    pub id: i64,
    pub username: String,
}

/// Full on-disk credentials file format.
/// Backwards compatible: `user`, `bots`, and `known_contacts` are optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialsFile {
    pub api_id: i32,
    pub api_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserIdentity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bots: Vec<BotEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_contacts: Vec<KnownContact>,
}

impl CredentialsFile {
    pub fn api_credentials(&self) -> ApiCredentials {
        ApiCredentials {
            api_id: self.api_id,
            api_hash: self.api_hash.clone(),
        }
    }

    pub fn find_bot_by_username(&self, username: &str) -> Option<&BotEntry> {
        let needle = username.strip_prefix('@').unwrap_or(username);
        self.bots
            .iter()
            .find(|b| b.username.eq_ignore_ascii_case(needle))
    }

    pub fn find_bot_by_id(&self, id: i64) -> Option<&BotEntry> {
        self.bots.iter().find(|b| b.id == id)
    }

    pub fn upsert_bot(&mut self, bot: BotEntry) {
        if let Some(existing) = self.bots.iter_mut().find(|b| b.id == bot.id) {
            *existing = bot;
        } else {
            self.bots.push(bot);
        }
    }

    /// Resolve a `@username` to a chat ID by checking user, bots, then known_contacts.
    pub fn resolve_username(&self, username: &str) -> Option<i64> {
        let needle = username.strip_prefix('@').unwrap_or(username);
        if let Some(ref user) = self.user
            && user
                .username
                .as_ref()
                .is_some_and(|u| u.eq_ignore_ascii_case(needle))
        {
            return Some(user.id);
        }
        if let Some(bot) = self.find_bot_by_username(needle) {
            return Some(bot.id);
        }
        self.known_contacts
            .iter()
            .find(|c| c.username.eq_ignore_ascii_case(needle))
            .map(|c| c.id)
    }

    pub fn upsert_known_contact(&mut self, contact: KnownContact) {
        if let Some(existing) = self
            .known_contacts
            .iter_mut()
            .find(|c| c.id == contact.id)
        {
            *existing = contact;
        } else {
            self.known_contacts.push(contact);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Env,
    Stored,
}

pub fn tg_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tg")
}

pub fn credentials_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CREDENTIALS_FILE)
}

pub fn load_credentials_for_auth(data_dir: &Path) -> Result<(ApiCredentials, CredentialSource)> {
    match env_credentials()? {
        Some(credentials) => Ok((credentials, CredentialSource::Env)),
        None => load_credentials_from_disk(data_dir)
            .map(|credentials| (credentials, CredentialSource::Stored)),
    }
}

pub fn load_credentials_for_non_auth(data_dir: &Path) -> Result<ApiCredentials> {
    load_credentials_from_disk(data_dir)
}

/// Load the full credentials file (with bots, known_contacts, etc.).
pub fn load_credentials_file(data_dir: &Path) -> Result<CredentialsFile> {
    let path = credentials_file_path(data_dir);
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            TgError::Other(format!(
                "API credentials not found at {}. Run `tg auth` first.",
                path.display()
            ))
        } else {
            TgError::Io(e)
        }
    })?;

    let creds_file: CredentialsFile = serde_json::from_str(&raw).map_err(|e| {
        TgError::Other(format!(
            "Failed to parse credentials at {}: {}",
            path.display(),
            e
        ))
    })?;

    Ok(creds_file)
}

/// Save the full credentials file.
pub fn save_credentials_file(creds_file: &CredentialsFile, data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = credentials_file_path(data_dir);
    let json = serde_json::to_string_pretty(creds_file)?;
    std::fs::write(&path, json)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

pub fn save_credentials(credentials: &ApiCredentials, data_dir: &Path) -> Result<()> {
    // Load existing file to preserve bots/contacts, or create a new one.
    let mut creds_file = load_credentials_file(data_dir).unwrap_or(CredentialsFile {
        api_id: credentials.api_id,
        api_hash: credentials.api_hash.clone(),
        user: None,
        bots: Vec::new(),
        known_contacts: Vec::new(),
    });
    creds_file.api_id = credentials.api_id;
    creds_file.api_hash = credentials.api_hash.clone();
    save_credentials_file(&creds_file, data_dir)
}

pub fn prompt_credentials() -> Result<ApiCredentials> {
    use std::io::{self, BufRead, Write};

    print!("Enter API ID (from my.telegram.org): ");
    io::stdout().flush().ok();
    let api_id_raw = io::stdin()
        .lock()
        .lines()
        .next()
        .ok_or_else(|| TgError::Other("Failed to read API ID".to_string()))?
        .map_err(|e| TgError::Other(e.to_string()))?;
    let api_id: i32 = api_id_raw
        .trim()
        .parse()
        .map_err(|_| TgError::Other("API ID must be a number".to_string()))?;

    print!("Enter API hash (from my.telegram.org): ");
    io::stdout().flush().ok();
    let api_hash = io::stdin()
        .lock()
        .lines()
        .next()
        .ok_or_else(|| TgError::Other("Failed to read API hash".to_string()))?
        .map_err(|e| TgError::Other(e.to_string()))?
        .trim()
        .to_string();

    if api_hash.is_empty() {
        return Err(TgError::Other("API hash cannot be empty".to_string()));
    }

    Ok(ApiCredentials { api_id, api_hash })
}

pub fn try_load_credentials_for_auth(
    data_dir: &Path,
) -> Option<(ApiCredentials, CredentialSource)> {
    load_credentials_for_auth(data_dir).ok()
}

fn load_credentials_from_disk(data_dir: &Path) -> Result<ApiCredentials> {
    let creds_file = load_credentials_file(data_dir)?;
    Ok(creds_file.api_credentials())
}

fn env_credentials() -> Result<Option<ApiCredentials>> {
    let api_id_env = std::env::var("TG_API_ID").ok();
    let api_hash_env = std::env::var("TG_API_HASH").ok();
    parse_env_credentials(api_id_env, api_hash_env)
}

fn parse_env_credentials(
    api_id_env: Option<String>,
    api_hash_env: Option<String>,
) -> Result<Option<ApiCredentials>> {
    if api_id_env.is_none() && api_hash_env.is_none() {
        return Ok(None);
    }

    let api_id_raw = api_id_env.ok_or_else(|| TgError::EnvVarMissing("TG_API_ID".to_string()))?;
    let api_hash = api_hash_env.ok_or_else(|| TgError::EnvVarMissing("TG_API_HASH".to_string()))?;

    let api_id = api_id_raw
        .parse()
        .map_err(|_| TgError::Other("TG_API_ID must be a number".to_string()))?;

    Ok(Some(ApiCredentials { api_id, api_hash }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_credentials_returns_none_when_absent() {
        let credentials = parse_env_credentials(None, None).unwrap();
        assert_eq!(credentials, None);
    }

    #[test]
    fn parse_env_credentials_requires_both_values() {
        let err = parse_env_credentials(Some("12345".to_string()), None).unwrap_err();
        assert!(err.to_string().contains("TG_API_HASH"));
    }

    #[test]
    fn parse_env_credentials_rejects_non_numeric_api_id() {
        let err =
            parse_env_credentials(Some("abc".to_string()), Some("hash".to_string())).unwrap_err();
        assert!(err.to_string().contains("TG_API_ID must be a number"));
    }

    #[test]
    fn parse_env_credentials_parses_valid_input() {
        let credentials =
            parse_env_credentials(Some("12345".to_string()), Some("hash".to_string())).unwrap();
        assert_eq!(
            credentials,
            Some(ApiCredentials {
                api_id: 12345,
                api_hash: "hash".to_string()
            })
        );
    }

    #[test]
    fn save_and_load_credentials_round_trip() {
        let tempdir = tempfile::tempdir().unwrap();
        let data_dir = tempdir.path().join("tg");

        let credentials = ApiCredentials {
            api_id: 12345,
            api_hash: "abc123".to_string(),
        };

        save_credentials(&credentials, &data_dir).unwrap();
        let loaded = load_credentials_from_disk(&data_dir).unwrap();

        assert_eq!(loaded, credentials);
    }

    #[test]
    fn backwards_compatible_with_old_credentials() {
        let tempdir = tempfile::tempdir().unwrap();
        let data_dir = tempdir.path().join("tg");
        std::fs::create_dir_all(&data_dir).unwrap();

        // Old format: just api_id and api_hash
        let old_json = r#"{"api_id": 12345, "api_hash": "abc123"}"#;
        std::fs::write(data_dir.join(CREDENTIALS_FILE), old_json).unwrap();

        let creds_file = load_credentials_file(&data_dir).unwrap();
        assert_eq!(creds_file.api_id, 12345);
        assert_eq!(creds_file.api_hash, "abc123");
        assert_eq!(creds_file.user, None);
        assert!(creds_file.bots.is_empty());
        assert!(creds_file.known_contacts.is_empty());
    }

    #[test]
    fn upsert_bot_adds_new() {
        let mut creds = CredentialsFile {
            api_id: 1,
            api_hash: "h".to_string(),
            user: None,
            bots: Vec::new(),
            known_contacts: Vec::new(),
        };
        creds.upsert_bot(BotEntry {
            id: 100,
            username: "testbot".to_string(),
            token: "100:AAA".to_string(),
        });
        assert_eq!(creds.bots.len(), 1);
        assert_eq!(creds.bots[0].username, "testbot");
    }

    #[test]
    fn upsert_bot_updates_existing() {
        let mut creds = CredentialsFile {
            api_id: 1,
            api_hash: "h".to_string(),
            user: None,
            bots: vec![BotEntry {
                id: 100,
                username: "testbot".to_string(),
                token: "100:AAA".to_string(),
            }],
            known_contacts: Vec::new(),
        };
        creds.upsert_bot(BotEntry {
            id: 100,
            username: "testbot".to_string(),
            token: "100:BBB".to_string(),
        });
        assert_eq!(creds.bots.len(), 1);
        assert_eq!(creds.bots[0].token, "100:BBB");
    }

    #[test]
    fn resolve_username_checks_user_bots_contacts() {
        let creds = CredentialsFile {
            api_id: 1,
            api_hash: "h".to_string(),
            user: Some(UserIdentity {
                id: 1,
                username: Some("myuser".to_string()),
            }),
            bots: vec![BotEntry {
                id: 2,
                username: "mybot".to_string(),
                token: "2:AAA".to_string(),
            }],
            known_contacts: vec![KnownContact {
                id: 3,
                username: "friend".to_string(),
            }],
        };

        assert_eq!(creds.resolve_username("@myuser"), Some(1));
        assert_eq!(creds.resolve_username("mybot"), Some(2));
        assert_eq!(creds.resolve_username("@friend"), Some(3));
        assert_eq!(creds.resolve_username("@unknown"), None);
    }

    #[test]
    fn find_bot_by_username_case_insensitive() {
        let creds = CredentialsFile {
            api_id: 1,
            api_hash: "h".to_string(),
            user: None,
            bots: vec![BotEntry {
                id: 100,
                username: "MyBot".to_string(),
                token: "100:AAA".to_string(),
            }],
            known_contacts: Vec::new(),
        };
        assert!(creds.find_bot_by_username("mybot").is_some());
        assert!(creds.find_bot_by_username("@MyBot").is_some());
    }

    #[test]
    fn save_preserves_bots_and_contacts() {
        let tempdir = tempfile::tempdir().unwrap();
        let data_dir = tempdir.path().join("tg");

        let creds_file = CredentialsFile {
            api_id: 12345,
            api_hash: "abc123".to_string(),
            user: Some(UserIdentity {
                id: 1,
                username: Some("testuser".to_string()),
            }),
            bots: vec![BotEntry {
                id: 100,
                username: "testbot".to_string(),
                token: "100:AAA".to_string(),
            }],
            known_contacts: vec![KnownContact {
                id: 200,
                username: "contact".to_string(),
            }],
        };

        save_credentials_file(&creds_file, &data_dir).unwrap();
        let loaded = load_credentials_file(&data_dir).unwrap();
        assert_eq!(loaded, creds_file);
    }

    #[test]
    fn save_api_credentials_preserves_existing_bots() {
        let tempdir = tempfile::tempdir().unwrap();
        let data_dir = tempdir.path().join("tg");

        // First save with bots
        let creds_file = CredentialsFile {
            api_id: 12345,
            api_hash: "abc123".to_string(),
            user: None,
            bots: vec![BotEntry {
                id: 100,
                username: "testbot".to_string(),
                token: "100:AAA".to_string(),
            }],
            known_contacts: Vec::new(),
        };
        save_credentials_file(&creds_file, &data_dir).unwrap();

        // Now save just API credentials
        let api_creds = ApiCredentials {
            api_id: 12345,
            api_hash: "abc123".to_string(),
        };
        save_credentials(&api_creds, &data_dir).unwrap();

        // Bots should still be there
        let loaded = load_credentials_file(&data_dir).unwrap();
        assert_eq!(loaded.bots.len(), 1);
        assert_eq!(loaded.bots[0].username, "testbot");
    }
}
