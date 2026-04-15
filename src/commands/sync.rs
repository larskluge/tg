use std::collections::HashMap;

use serde::Serialize;

use crate::client::{BoundaryResult, TelegramClient};
use crate::output::MessageInfo;

/// Milliseconds to wait before a single retry of `get_boundary_message_id`.
const BOUNDARY_RETRY_DELAY_MS: u64 = 300;

/// Per-chat sync outcome: either messages or an error description.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SyncResult {
    Messages(Vec<MessageInfo>),
    Error { error: String },
}

/// Parse stdin JSON into a map of chat_id -> HWM timestamp string.
pub fn parse_hwm_input(input: &str) -> std::result::Result<HashMap<i64, String>, String> {
    let raw: HashMap<String, String> =
        serde_json::from_str(input).map_err(|e| format!("Invalid JSON: {e}"))?;
    let mut result = HashMap::new();
    for (key, value) in raw {
        let chat_id: i64 = key
            .parse()
            .map_err(|_| format!("Invalid chat ID: {key}"))?;
        result.insert(chat_id, value);
    }
    Ok(result)
}

pub async fn sync_chats<C: TelegramClient>(
    client: &C,
    hwm_map: HashMap<i64, String>,
    limit: i32,
    reconcile_days: Option<u32>,
) -> HashMap<i64, SyncResult> {
    let effective_hwm_map: HashMap<i64, String> = if let Some(days) = reconcile_days {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        hwm_map
            .into_iter()
            .map(|(id, _)| (id, cutoff.to_rfc3339()))
            .collect()
    } else {
        hwm_map
    };

    let mut results = HashMap::new();
    for (chat_id, hwm) in effective_hwm_map {
        let result = sync_single_chat(client, chat_id, &hwm, limit).await;
        results.insert(chat_id, result);
    }
    results
}

async fn sync_single_chat<C: TelegramClient>(
    client: &C,
    chat_id: i64,
    hwm_str: &str,
    limit: i32,
) -> SyncResult {
    let timestamp = match crate::commands::messages::parse_since_date(hwm_str) {
        Ok(ts) => ts,
        Err(e) => return SyncResult::Error { error: e.to_string() },
    };

    // Warmup fetch to trigger TDLib server sync
    if let Err(e) = client.get_messages(chat_id, 1, None).await {
        return SyncResult::Error { error: e.to_string() };
    }

    let boundary = match client.get_boundary_message_id(chat_id, timestamp).await {
        Ok(b) => b,
        Err(e) => return SyncResult::Error { error: e.to_string() },
    };

    let (until_message_id, no_boundary) = match boundary {
        BoundaryResult::Empty => return SyncResult::Messages(vec![]),
        BoundaryResult::BoundAt(id) => (Some(id), false),
        BoundaryResult::None => {
            // Retry once after delay
            tokio::time::sleep(tokio::time::Duration::from_millis(BOUNDARY_RETRY_DELAY_MS)).await;
            match client.get_boundary_message_id(chat_id, timestamp).await {
                Ok(BoundaryResult::BoundAt(id)) => (Some(id), false),
                Ok(_) => (None, true),
                Err(e) => return SyncResult::Error { error: e.to_string() },
            }
        }
    };

    let messages = match client.get_messages(chat_id, limit, until_message_id).await {
        Ok(msgs) => msgs,
        Err(e) => return SyncResult::Error { error: e.to_string() },
    };

    // If boundary was None (no cutoff), filter by timestamp
    let messages = if no_boundary {
        messages
            .into_iter()
            .filter(|m| m.timestamp >= timestamp)
            .collect()
    } else {
        messages
    };

    SyncResult::Messages(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    fn make_message(id: i64, chat_id: i64, timestamp: i32) -> MessageInfo {
        MessageInfo {
            id,
            chat_id,
            sender_id: Some(100),
            sender: "Alice".to_string(),
            text: format!("msg {id}"),
            date: "1h ago".to_string(),
            timestamp,
            is_outgoing: false,
            edit_date: None,
            content_type: Some("text".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: None,
        }
    }

    #[test]
    fn parse_hwm_valid() {
        let input = r#"{"123": "2026-01-01T00:00:00Z", "-456": "2026-06-01T00:00:00Z"}"#;
        let result = parse_hwm_input(input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[&123], "2026-01-01T00:00:00Z");
        assert_eq!(result[&-456], "2026-06-01T00:00:00Z");
    }

    #[test]
    fn parse_hwm_empty_object() {
        let result = parse_hwm_input("{}").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_hwm_invalid_json() {
        let err = parse_hwm_input("not json").unwrap_err();
        assert!(err.contains("Invalid JSON"), "expected 'Invalid JSON' in: {err}");
    }

    #[test]
    fn parse_hwm_non_numeric_chat_id() {
        let input = r#"{"abc": "2026-01-01T00:00:00Z"}"#;
        let err = parse_hwm_input(input).unwrap_err();
        assert!(err.contains("Invalid chat ID"), "expected 'Invalid chat ID' in: {err}");
    }

    #[tokio::test]
    async fn sync_happy_path_multiple_chats() {
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            messages: vec![
                make_message(1, 1, 1000),
                make_message(2, 1, 2000),
            ],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, "2026-01-01T00:00:00Z".to_string());
        hwm_map.insert(2i64, "2026-01-01T00:00:00Z".to_string());

        let results = sync_chats(&client, hwm_map, 20, None).await;
        assert_eq!(results.len(), 2);

        for (_chat_id, result) in &results {
            match result {
                SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
                SyncResult::Error { error } => panic!("unexpected error: {error}"),
            }
        }
    }

    #[tokio::test]
    async fn sync_empty_hwm_map() {
        let client = MockClient::default();
        let results = sync_chats(&client, HashMap::new(), 20, None).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn sync_partial_failure() {
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            inaccessible_chat_ids: vec![999],
            messages: vec![make_message(1, 1, 1000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, "2026-01-01T00:00:00Z".to_string());
        hwm_map.insert(999i64, "2026-01-01T00:00:00Z".to_string());

        let results = sync_chats(&client, hwm_map, 20, None).await;
        assert_eq!(results.len(), 2);

        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
            SyncResult::Error { error } => panic!("chat 1 should succeed, got: {error}"),
        }

        match &results[&999] {
            SyncResult::Error { .. } => {} // expected
            SyncResult::Messages(_) => panic!("chat 999 should fail"),
        }
    }

    #[tokio::test]
    async fn sync_boundary_empty_returns_empty_messages() {
        let client = MockClient {
            boundary_result: BoundaryResult::Empty,
            messages: vec![make_message(1, 1, 1000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, "2037-01-01T00:00:00Z".to_string());

        let results = sync_chats(&client, hwm_map, 20, None).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(msgs.is_empty()),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[tokio::test]
    async fn sync_reconcile_days_overrides_hwm() {
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            messages: vec![make_message(1, 1, 1000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        // Far-future HWM that would result in empty if not overridden
        hwm_map.insert(1i64, "2099-01-01T00:00:00Z".to_string());

        // reconcile_days=7 should override to ~7 days ago, giving non-empty results
        let results = sync_chats(&client, hwm_map, 20, Some(7)).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[tokio::test]
    async fn sync_invalid_hwm_timestamp() {
        let client = MockClient::default();

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, "not-a-date".to_string());

        let results = sync_chats(&client, hwm_map, 20, None).await;
        match &results[&1] {
            SyncResult::Error { error } => {
                assert!(
                    error.contains("Invalid date format"),
                    "expected 'Invalid date format' in: {error}"
                );
            }
            SyncResult::Messages(_) => panic!("expected error for invalid date"),
        }
    }
}
