use crate::error::{Result, TgError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CREDENTIALS_FILE: &str = "credentials.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiCredentials {
    pub api_id: i32,
    pub api_hash: String,
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

pub fn save_credentials(credentials: &ApiCredentials, data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;

    let path = credentials_file_path(data_dir);
    let json = serde_json::to_string_pretty(credentials)?;
    std::fs::write(&path, json)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
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

    let credentials: ApiCredentials = serde_json::from_str(&raw).map_err(|e| {
        TgError::Other(format!(
            "Failed to parse API credentials at {}: {}",
            path.display(),
            e
        ))
    })?;

    Ok(credentials)
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
}
