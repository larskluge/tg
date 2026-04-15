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

/// Parse stdin JSON into a map of chat_id -> last seen message ID.
///
/// Input format: `{"chat_id": last_message_id, ...}`
/// Example: `{"-1001666847309": 89508544512, "123456789": 42}`
///
/// A value of `0` means "fetch all messages" (no prior HWM).
pub fn parse_hwm_input(input: &str) -> std::result::Result<HashMap<i64, i64>, String> {
    let raw: HashMap<String, i64> =
        serde_json::from_str(input).map_err(|e| format!("Invalid JSON: {e}"))?;
    let mut result = HashMap::new();
    for (key, value) in raw {
        let chat_id: i64 = key.parse().map_err(|_| format!("Invalid chat ID: {key}"))?;
        result.insert(chat_id, value);
    }
    Ok(result)
}

/// Bulk-sync messages for multiple chats within a single TDLib session.
///
/// For each chat in `hwm_map`, fetches messages newer than the last seen message ID.
/// If `reconcile_days` is set, all HWMs are overridden with a message-ID boundary
/// computed from `now - N days`.
pub async fn sync_chats<C: TelegramClient>(
    client: &C,
    hwm_map: HashMap<i64, i64>,
    limit: i32,
    reconcile_days: Option<u32>,
) -> HashMap<i64, SyncResult> {
    let mut results = HashMap::new();

    if let Some(days) = reconcile_days {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let timestamp = cutoff.timestamp() as i32;

        for &chat_id in hwm_map.keys() {
            let result =
                sync_single_chat_by_timestamp(client, chat_id, timestamp, limit).await;
            results.insert(chat_id, result);
        }
    } else {
        for (chat_id, hwm_message_id) in hwm_map {
            let result = sync_single_chat(client, chat_id, hwm_message_id, limit).await;
            results.insert(chat_id, result);
        }
    }

    results
}

/// Fetch messages newer than `hwm_message_id` for a single chat.
///
/// Uses `hwm_message_id` as the inclusive lower boundary for `get_messages`,
/// then strips the boundary message itself (it was already ingested).
/// A `hwm_message_id` of 0 means "no prior state — fetch latest messages".
async fn sync_single_chat<C: TelegramClient>(
    client: &C,
    chat_id: i64,
    hwm_message_id: i64,
    limit: i32,
) -> SyncResult {
    let until = if hwm_message_id > 0 {
        Some(hwm_message_id)
    } else {
        None
    };

    match client.get_messages(chat_id, limit, until).await {
        Ok(mut messages) => {
            // Drop the boundary message itself — it was already ingested
            if hwm_message_id > 0 {
                messages.retain(|m| m.id != hwm_message_id);
            }
            SyncResult::Messages(messages)
        }
        Err(e) => SyncResult::Error {
            error: e.to_string(),
        },
    }
}

/// Fetch messages newer than a timestamp for a single chat (used by --reconcile-days).
///
/// Falls back to timestamp-based boundary lookup since we don't have a message ID.
async fn sync_single_chat_by_timestamp<C: TelegramClient>(
    client: &C,
    chat_id: i64,
    timestamp: i32,
    limit: i32,
) -> SyncResult {
    // Warmup fetch to trigger TDLib server sync
    if let Err(e) = client.get_messages(chat_id, 1, None).await {
        return SyncResult::Error {
            error: e.to_string(),
        };
    }

    let boundary = match client.get_boundary_message_id(chat_id, timestamp).await {
        Ok(b) => b,
        Err(e) => {
            return SyncResult::Error {
                error: e.to_string(),
            };
        }
    };

    let (until_message_id, no_boundary) = match boundary {
        BoundaryResult::Empty => return SyncResult::Messages(vec![]),
        BoundaryResult::BoundAt(id) => (Some(id), false),
        BoundaryResult::None => {
            tokio::time::sleep(tokio::time::Duration::from_millis(BOUNDARY_RETRY_DELAY_MS)).await;
            match client.get_boundary_message_id(chat_id, timestamp).await {
                Ok(BoundaryResult::BoundAt(id)) => (Some(id), false),
                Ok(_) => (None, true),
                Err(e) => {
                    return SyncResult::Error {
                        error: e.to_string(),
                    };
                }
            }
        }
    };

    match client.get_messages(chat_id, limit, until_message_id).await {
        Ok(messages) => {
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
        Err(e) => SyncResult::Error {
            error: e.to_string(),
        },
    }
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

    // --- parse_hwm_input tests ---

    #[test]
    fn parse_hwm_valid() {
        let input = r#"{"123": 42, "-1001666847309": 89508544512}"#;
        let result = parse_hwm_input(input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[&123], 42);
        assert_eq!(result[&-1001666847309], 89508544512);
    }

    #[test]
    fn parse_hwm_zero_means_no_hwm() {
        let input = r#"{"123": 0}"#;
        let result = parse_hwm_input(input).unwrap();
        assert_eq!(result[&123], 0);
    }

    #[test]
    fn parse_hwm_empty_object() {
        let result = parse_hwm_input("{}").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_hwm_invalid_json() {
        let err = parse_hwm_input("not json").unwrap_err();
        assert!(
            err.contains("Invalid JSON"),
            "expected 'Invalid JSON' in: {err}"
        );
    }

    #[test]
    fn parse_hwm_non_numeric_chat_id() {
        let input = r#"{"abc": 42}"#;
        let err = parse_hwm_input(input).unwrap_err();
        assert!(
            err.contains("Invalid chat ID"),
            "expected 'Invalid chat ID' in: {err}"
        );
    }

    // --- sync_chats tests ---

    #[tokio::test]
    async fn sync_happy_path_multiple_chats() {
        let client = MockClient {
            messages: vec![make_message(10, 1, 1000), make_message(20, 1, 2000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, 5i64); // HWM at msg 5, should get msgs 10 and 20
        hwm_map.insert(2i64, 5i64);

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
    async fn sync_strips_boundary_message() {
        // If HWM is msg 10, and results include msg 10 (the boundary), it should be stripped
        let client = MockClient {
            messages: vec![make_message(10, 1, 1000), make_message(20, 1, 2000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, 10i64); // HWM is msg 10 itself

        let results = sync_chats(&client, hwm_map, 20, None).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => {
                assert!(
                    !msgs.iter().any(|m| m.id == 10),
                    "boundary message (id=10) should be stripped"
                );
                assert!(
                    msgs.iter().any(|m| m.id == 20),
                    "newer message (id=20) should be included"
                );
            }
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[tokio::test]
    async fn sync_hwm_zero_fetches_all() {
        let client = MockClient {
            messages: vec![make_message(1, 1, 1000), make_message(2, 1, 2000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, 0i64); // No prior HWM

        let results = sync_chats(&client, hwm_map, 20, None).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert_eq!(msgs.len(), 2),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
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
            inaccessible_chat_ids: vec![999],
            messages: vec![make_message(1, 1, 1000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, 0i64);
        hwm_map.insert(999i64, 0i64);

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
    async fn sync_reconcile_days_uses_timestamp_path() {
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            messages: vec![make_message(1, 1, 1000)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, 99999i64); // message ID ignored when reconcile_days is set

        let results = sync_chats(&client, hwm_map, 20, Some(7)).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    // --- serialization tests ---

    #[test]
    fn sync_result_messages_serializes_as_array() {
        let result = SyncResult::Messages(vec![make_message(1, 1, 1000)]);
        let json = serde_json::to_value(&result).unwrap();
        assert!(
            json.is_array(),
            "Messages variant should serialize as JSON array"
        );
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    #[test]
    fn sync_result_empty_messages_serializes_as_empty_array() {
        let result = SyncResult::Messages(vec![]);
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.is_array());
        assert!(json.as_array().unwrap().is_empty());
    }

    #[test]
    fn sync_result_error_serializes_as_object() {
        let result = SyncResult::Error {
            error: "Chat not found".to_string(),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert!(
            json.is_object(),
            "Error variant should serialize as JSON object"
        );
        assert_eq!(json["error"], "Chat not found");
    }

    #[test]
    fn full_sync_output_serializes_correctly() {
        let mut results: HashMap<i64, SyncResult> = HashMap::new();
        results.insert(123, SyncResult::Messages(vec![make_message(1, 123, 1000)]));
        results.insert(456, SyncResult::Messages(vec![]));
        results.insert(
            999,
            SyncResult::Error {
                error: "Not found".to_string(),
            },
        );

        let json = serde_json::to_value(&results).unwrap();
        assert!(json["123"].is_array());
        assert_eq!(json["123"].as_array().unwrap().len(), 1);
        assert!(json["456"].is_array());
        assert!(json["456"].as_array().unwrap().is_empty());
        assert_eq!(json["999"]["error"], "Not found");
    }
}
