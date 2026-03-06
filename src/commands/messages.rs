use crate::client::TelegramClient;
use crate::error::{Result, TgError};
use crate::output::MessageInfo;

pub enum ChatTarget {
    Id(i64),
    Name(String),
}

/// Parse a `YYYY-MM-DD` date string as start-of-day UTC and return the Unix timestamp (i32).
pub fn parse_since_date(s: &str) -> Result<i32> {
    let date = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| TgError::Other(format!("Invalid date format '{}'. Expected YYYY-MM-DD", s)))?;
    let dt = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| TgError::Other("Failed to construct datetime".to_string()))?;
    let ts = dt.and_utc().timestamp();
    i32::try_from(ts)
        .map_err(|_| TgError::Other(format!("Date '{}' out of range for TDLib timestamp", s)))
}

pub async fn get_messages<C: TelegramClient>(
    client: &C,
    target: ChatTarget,
    limit: i32,
    since_utc: Option<&str>,
) -> Result<Vec<MessageInfo>> {
    let chat_id = match target {
        ChatTarget::Id(id) => id,
        ChatTarget::Name(name) => client.find_chat_by_name(&name).await?,
    };

    let until_message_id = if let Some(date_str) = since_utc {
        let timestamp = parse_since_date(date_str)?;
        client.get_boundary_message_id(chat_id, timestamp).await?
    } else {
        None
    };

    client.get_messages(chat_id, limit, until_message_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[test]
    fn parse_since_date_valid() {
        let ts = parse_since_date("2026-03-01").unwrap();
        // 2026-03-01 00:00:00 UTC
        assert_eq!(ts, 1772323200);
    }

    #[test]
    fn parse_since_date_epoch() {
        let ts = parse_since_date("1970-01-01").unwrap();
        assert_eq!(ts, 0);
    }

    #[test]
    fn parse_since_date_invalid_format() {
        assert!(parse_since_date("03-01-2026").is_err());
        assert!(parse_since_date("not-a-date").is_err());
        assert!(parse_since_date("2026/03/01").is_err());
    }

    #[tokio::test]
    async fn get_messages_by_id() {
        let client = MockClient::default();
        let messages = get_messages(&client, ChatTarget::Id(1), 20, None)
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn get_messages_by_name() {
        let client = MockClient::default();
        let messages = get_messages(&client, ChatTarget::Name("John".to_string()), 20, None)
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn get_messages_respects_limit() {
        let client = MockClient::default();
        let messages = get_messages(&client, ChatTarget::Id(1), 1, None)
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
    }
}
