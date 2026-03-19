use crate::client::TelegramClient;
use crate::error::{Result, TgError};
use crate::output::MessageInfo;

pub enum ChatTarget {
    Id(i64),
    Name(String),
}

/// Parse a date or datetime string and return the Unix timestamp (i32).
///
/// Accepts:
/// - Full ISO 8601: `2026-03-18T09:34:05Z`
/// - Date only: `2026-03-18` (treated as midnight UTC)
pub fn parse_since_date(s: &str) -> Result<i32> {
    let ts = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        dt.timestamp()
    } else if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| TgError::Other("Failed to construct datetime".to_string()))?;
        dt.and_utc().timestamp()
    } else {
        return Err(TgError::Other(format!(
            "Invalid date format '{}'. Expected YYYY-MM-DD or ISO 8601 (e.g. 2026-03-18T09:34:05Z)",
            s
        )));
    };
    i32::try_from(ts)
        .map_err(|_| TgError::Other(format!("Date '{}' out of range for TDLib timestamp", s)))
}

#[derive(Debug)]
pub struct MessagesResult {
    pub chat_id: i64,
    pub messages: Vec<MessageInfo>,
}

pub async fn get_messages<C: TelegramClient>(
    client: &C,
    target: ChatTarget,
    limit: i32,
    since_utc: Option<&str>,
    oldest_first: bool,
) -> Result<MessagesResult> {
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

    let messages = if oldest_first {
        // Fetch all messages by using i32::MAX as the limit, then reverse
        let mut all = client
            .get_messages(chat_id, i32::MAX, until_message_id)
            .await?;
        all.reverse();
        all.truncate(limit as usize);
        all
    } else {
        client.get_messages(chat_id, limit, until_message_id).await?
    };
    Ok(MessagesResult { chat_id, messages })
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
    fn parse_since_date_iso8601() {
        // 2026-03-18T09:34:05Z
        let ts = parse_since_date("2026-03-18T09:34:05Z").unwrap();
        assert_eq!(ts, 1773826445);
    }

    #[test]
    fn parse_since_date_iso8601_with_offset() {
        // 2026-03-18T09:34:05+00:00 is equivalent to Z
        let ts = parse_since_date("2026-03-18T09:34:05+00:00").unwrap();
        assert_eq!(ts, 1773826445);
    }

    #[test]
    fn parse_since_date_invalid_format() {
        assert!(parse_since_date("03-01-2026").is_err());
        assert!(parse_since_date("not-a-date").is_err());
        assert!(parse_since_date("2026/03/01").is_err());
    }

    #[test]
    fn parse_since_date_invalid_error_message() {
        let err = parse_since_date("bad-input").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bad-input"));
        assert!(msg.contains("ISO 8601"));
    }

    #[tokio::test]
    async fn get_messages_by_id() {
        let client = MockClient::default();
        let result = get_messages(&client, ChatTarget::Id(1), 20, None, false)
            .await
            .unwrap();
        assert_eq!(result.chat_id, 1);
        assert_eq!(result.messages.len(), 2);
    }

    #[tokio::test]
    async fn get_messages_by_name() {
        let client = MockClient::default();
        let result = get_messages(&client, ChatTarget::Name("John".to_string()), 20, None, false)
            .await
            .unwrap();
        assert_eq!(result.chat_id, 1);
        assert_eq!(result.messages.len(), 2);
    }

    #[tokio::test]
    async fn get_messages_respects_limit() {
        let client = MockClient::default();
        let result = get_messages(&client, ChatTarget::Id(1), 1, None, false)
            .await
            .unwrap();
        assert_eq!(result.messages.len(), 1);
    }

    #[tokio::test]
    async fn no_messages_returns_ok_with_empty_vec() {
        let client = MockClient {
            messages: vec![],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 20, Some("2026-03-01"), false)
            .await
            .unwrap();
        assert_eq!(result.chat_id, 1);
        assert!(result.messages.is_empty());
    }

    #[tokio::test]
    async fn inaccessible_chat_returns_error() {
        let client = MockClient {
            inaccessible_chat_ids: vec![999],
            ..MockClient::default()
        };
        let err = get_messages(&client, ChatTarget::Id(999), 20, None, false)
            .await
            .unwrap_err();
        match err {
            TgError::ChatInaccessible(id) => assert_eq!(id, 999),
            other => panic!("Expected ChatInaccessible, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn oldest_first_reverses_messages() {
        let client = MockClient::default();
        // Default mock returns messages as [id=1, id=2]; reversal gives [id=2, id=1]
        let normal = get_messages(&client, ChatTarget::Id(1), 20, None, false)
            .await
            .unwrap();
        let reversed = get_messages(&client, ChatTarget::Id(1), 20, None, true)
            .await
            .unwrap();
        assert_eq!(reversed.messages.len(), normal.messages.len());
        assert_eq!(reversed.messages[0].id, normal.messages[1].id);
        assert_eq!(reversed.messages[1].id, normal.messages[0].id);
    }

    #[tokio::test]
    async fn oldest_first_with_limit_caps_result() {
        let client = MockClient::default();
        // Default mock has 2 messages; oldest_first with limit=1 should return 1 message
        let result = get_messages(&client, ChatTarget::Id(1), 1, None, true)
            .await
            .unwrap();
        assert_eq!(result.messages.len(), 1);
    }

    #[tokio::test]
    async fn since_utc_date_only_is_inclusive() {
        // Mock has messages with id=1 and id=2; set boundary to id=1
        // Inclusive behavior: id=1 (boundary) AND id=2 (newer) should both be returned
        let client = MockClient {
            boundary_message_id: Some(1),
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 20, Some("2026-01-01"), false)
            .await
            .unwrap();
        assert!(
            result.messages.iter().any(|m| m.id == 1),
            "boundary message (id=1) should be included (inclusive)"
        );
        assert!(
            result.messages.iter().any(|m| m.id == 2),
            "newer message (id=2) should be included"
        );
    }

    #[tokio::test]
    async fn since_utc_iso8601_with_time_is_inclusive() {
        // Tests that a full ISO 8601 timestamp (with time component) is accepted
        // and that the message at exactly that timestamp is included.
        // The mock maps the parsed timestamp to boundary id=1 regardless of value,
        // so this verifies both: (a) the full datetime string is parsed without error,
        // and (b) the boundary message is included in results.
        let client = MockClient {
            boundary_message_id: Some(1),
            ..MockClient::default()
        };
        // Full ISO 8601 with time — would fail in the old date-only parser
        let result = get_messages(
            &client,
            ChatTarget::Id(1),
            20,
            Some("2026-03-18T09:34:05Z"),
            false,
        )
        .await
        .unwrap();
        assert!(
            result.messages.iter().any(|m| m.id == 1),
            "boundary message (id=1) should be included with ISO 8601 timestamp"
        );
        assert!(
            result.messages.iter().any(|m| m.id == 2),
            "newer message (id=2) should be included"
        );
    }

    #[tokio::test]
    async fn oldest_first_empty_chat() {
        let client = MockClient {
            messages: vec![],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 20, None, true)
            .await
            .unwrap();
        assert!(result.messages.is_empty());
    }
}
