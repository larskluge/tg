use crate::client::{TdLibClient, TelegramClient};
use crate::credentials::{self, CredentialsFile, KnownContact};
use crate::error::{Result, TgError};
use std::path::Path;

/// Parsed recipient from CLI args, decoupled from clap types.
pub enum Recipient {
    To(String),
    Id(i64),
    Group(String),
    Name(String),
}

pub async fn resolve_recipient(
    recipient: Recipient,
    creds_file: &CredentialsFile,
    data_dir: &Path,
) -> Result<i64> {
    match recipient {
        Recipient::To(to) => resolve_to(&to, creds_file, data_dir).await,
        Recipient::Id(id) => Ok(id),
        Recipient::Group(group) => resolve_via_tdlib_group(&group, creds_file).await,
        Recipient::Name(name) => resolve_name(&name, creds_file, data_dir).await,
    }
}

async fn resolve_to(to: &str, creds_file: &CredentialsFile, data_dir: &Path) -> Result<i64> {
    if let Ok(id) = to.parse::<i64>() {
        return Ok(id);
    }
    if to.starts_with('@') {
        if let Some(id) = creds_file.resolve_username(to) {
            return Ok(id);
        }
        return resolve_username_via_tdlib(to, creds_file, data_dir).await;
    }
    resolve_via_tdlib_name(to, creds_file).await
}

async fn resolve_name(name: &str, creds_file: &CredentialsFile, data_dir: &Path) -> Result<i64> {
    if name.starts_with('@') {
        if let Some(id) = creds_file.resolve_username(name) {
            return Ok(id);
        }
        return resolve_username_via_tdlib(name, creds_file, data_dir).await;
    }
    resolve_via_tdlib_name(name, creds_file).await
}

fn require_api_credentials(creds_file: &CredentialsFile, context: &str) -> Result<(i32, String)> {
    let api_creds = creds_file.api_credentials();
    if api_creds.api_id == 0 {
        return Err(TgError::Other(format!(
            "Cannot resolve {context}. Run `tg auth` first or use a numeric ID."
        )));
    }
    Ok((api_creds.api_id, api_creds.api_hash))
}

async fn resolve_via_tdlib_name(name: &str, creds_file: &CredentialsFile) -> Result<i64> {
    let (api_id, api_hash) = require_api_credentials(creds_file, &format!("\"{name}\""))?;
    let mut client = TdLibClient::new(api_id, api_hash)?;
    client.start().await?;
    let chat_id = client.find_chat_by_name(name).await;
    client.shutdown().await;
    chat_id
}

async fn resolve_via_tdlib_group(group: &str, creds_file: &CredentialsFile) -> Result<i64> {
    let (api_id, api_hash) = require_api_credentials(creds_file, &format!("group \"{group}\""))?;
    let mut client = TdLibClient::new(api_id, api_hash)?;
    client.start().await?;
    let chat_id = client.find_group_by_name(group).await;
    client.shutdown().await;
    chat_id
}

async fn resolve_username_via_tdlib(
    username: &str,
    creds_file: &CredentialsFile,
    data_dir: &Path,
) -> Result<i64> {
    let (api_id, api_hash) = require_api_credentials(creds_file, username)?;
    let mut client = TdLibClient::new(api_id, api_hash)?;
    client.start().await?;

    let needle = username.strip_prefix('@').unwrap_or(username);
    let chat_id = client.find_chat_by_username(needle).await;
    client.shutdown().await;

    let chat_id = chat_id?;

    // Cache the resolved contact
    let mut updated_creds = credentials::load_credentials_file(data_dir)?;
    updated_creds.upsert_known_contact(KnownContact {
        id: chat_id,
        username: needle.to_string(),
    });
    credentials::save_credentials_file(&updated_creds, data_dir)?;

    Ok(chat_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{BotEntry, KnownContact, UserIdentity};

    fn test_creds() -> CredentialsFile {
        CredentialsFile {
            api_id: 0,
            api_hash: String::new(),
            user: Some(UserIdentity {
                id: 100,
                username: Some("testuser".to_string()),
            }),
            bots: vec![BotEntry {
                id: 200,
                username: "testbot".to_string(),
                token: "200:AAA".to_string(),
            }],
            known_contacts: vec![KnownContact {
                id: 300,
                username: "knownfriend".to_string(),
            }],
        }
    }

    #[tokio::test]
    async fn resolve_numeric_to() {
        let creds = test_creds();
        let tempdir = tempfile::tempdir().unwrap();
        let id = resolve_recipient(Recipient::To("12345".to_string()), &creds, tempdir.path())
            .await
            .unwrap();
        assert_eq!(id, 12345);
    }

    #[tokio::test]
    async fn resolve_negative_numeric_to() {
        let creds = test_creds();
        let tempdir = tempfile::tempdir().unwrap();
        let id = resolve_recipient(
            Recipient::To("-1001234567890".to_string()),
            &creds,
            tempdir.path(),
        )
        .await
        .unwrap();
        assert_eq!(id, -1001234567890);
    }

    #[tokio::test]
    async fn resolve_id_directly() {
        let creds = test_creds();
        let tempdir = tempfile::tempdir().unwrap();
        let id = resolve_recipient(Recipient::Id(999), &creds, tempdir.path())
            .await
            .unwrap();
        assert_eq!(id, 999);
    }

    #[tokio::test]
    async fn resolve_cached_username_via_to() {
        let creds = test_creds();
        let tempdir = tempfile::tempdir().unwrap();
        let id = resolve_recipient(
            Recipient::To("@knownfriend".to_string()),
            &creds,
            tempdir.path(),
        )
        .await
        .unwrap();
        assert_eq!(id, 300);
    }

    #[tokio::test]
    async fn resolve_cached_bot_username_via_to() {
        let creds = test_creds();
        let tempdir = tempfile::tempdir().unwrap();
        let id = resolve_recipient(
            Recipient::To("@testbot".to_string()),
            &creds,
            tempdir.path(),
        )
        .await
        .unwrap();
        assert_eq!(id, 200);
    }

    #[tokio::test]
    async fn resolve_cached_user_username_via_name() {
        let creds = test_creds();
        let tempdir = tempfile::tempdir().unwrap();
        let id = resolve_recipient(
            Recipient::Name("@testuser".to_string()),
            &creds,
            tempdir.path(),
        )
        .await
        .unwrap();
        assert_eq!(id, 100);
    }

    #[tokio::test]
    async fn resolve_unknown_username_without_auth_errors() {
        let creds = test_creds(); // api_id == 0
        let tempdir = tempfile::tempdir().unwrap();
        let err = resolve_recipient(
            Recipient::To("@unknown".to_string()),
            &creds,
            tempdir.path(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Run `tg auth` first"));
    }

    #[tokio::test]
    async fn resolve_name_without_auth_errors() {
        let creds = test_creds(); // api_id == 0
        let tempdir = tempfile::tempdir().unwrap();
        let err = resolve_recipient(
            Recipient::To("Some Person".to_string()),
            &creds,
            tempdir.path(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Run `tg auth` first"));
    }

    #[tokio::test]
    async fn resolve_group_without_auth_errors() {
        let creds = test_creds(); // api_id == 0
        let tempdir = tempfile::tempdir().unwrap();
        let err = resolve_recipient(
            Recipient::Group("Family".to_string()),
            &creds,
            tempdir.path(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Run `tg auth` first"));
    }
}
