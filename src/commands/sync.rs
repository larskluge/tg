use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::client::{BoundaryResult, TelegramClient};
use crate::error::{Result, TgError};
use crate::output::MessageInfo;

fn default_sync_limit() -> i32 {
    1000
}

/// Wire-format request for `sync`. The HWM map is keyed by stringified chat
/// ID because JSON object keys must be strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    #[serde(default)]
    pub hwm: HashMap<String, i64>,
    #[serde(default = "default_sync_limit")]
    pub limit: i32,
    #[serde(default)]
    pub reconcile_days: Option<u32>,
    /// Return the OLDEST `limit` messages above each HWM, ascending, instead
    /// of the newest `limit`, descending. See [`SyncMode::OldestFirst`].
    #[serde(default)]
    pub oldest_first: bool,
}

impl Default for SyncRequest {
    fn default() -> Self {
        Self {
            hwm: HashMap::new(),
            limit: default_sync_limit(),
            reconcile_days: None,
            oldest_first: false,
        }
    }
}

/// Which messages a sync returns for each chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// The NEWEST `limit` messages above each HWM, newest-first. The original
    /// contract, unchanged. With more than `limit` messages above the HWM the
    /// older ones are not returned at all, so a caller that advances its HWM
    /// to the highest id it received skips them.
    NewestFirst,
    /// The OLDEST `limit` messages above each HWM, oldest-first (ascending
    /// id), with memory bounded by `limit`. A caller that advances its HWM to
    /// the last id returned and asks again pages through any gap without
    /// losing a message. HWM `0` starts at the chat's first message.
    OldestFirst,
    /// Every chat from now minus `days`, newest-first; the HWMs are ignored.
    Reconcile { days: u32 },
}

impl SyncMode {
    /// The mode a request's two flags ask for. `oldest_first` with
    /// `reconcile_days` is refused rather than resolved: a reconcile sweep reads
    /// newest-first by design, and dropping either flag would answer with a
    /// window the caller did not ask for.
    pub fn new(reconcile_days: Option<u32>, oldest_first: bool) -> Result<Self> {
        match (reconcile_days, oldest_first) {
            (Some(_), true) => Err(TgError::Other(
                "oldest_first cannot be combined with reconcile_days (a reconcile sweep is newest-first)"
                    .to_string(),
            )),
            (Some(days), false) => Ok(Self::Reconcile { days }),
            (None, true) => Ok(Self::OldestFirst),
            (None, false) => Ok(Self::NewestFirst),
        }
    }
}

pub async fn handle<C: TelegramClient>(
    client: &C,
    req: SyncRequest,
) -> Result<HashMap<i64, SyncResult>> {
    let mode = SyncMode::new(req.reconcile_days, req.oldest_first)?;
    let mut hwm_map = HashMap::with_capacity(req.hwm.len());
    for (k, v) in req.hwm {
        let id: i64 = k
            .parse()
            .map_err(|_| TgError::Other(format!("Invalid chat ID: {k}")))?;
        hwm_map.insert(id, v);
    }
    Ok(sync_chats(client, hwm_map, req.limit, mode).await)
}

/// Per-chat sync outcome: either messages or an error description.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SyncResult {
    Messages(Vec<MessageInfo>),
    Error { error: String },
}

impl From<Result<Vec<MessageInfo>>> for SyncResult {
    fn from(result: Result<Vec<MessageInfo>>) -> Self {
        match result {
            Ok(messages) => Self::Messages(messages),
            Err(e) => Self::Error {
                error: e.to_string(),
            },
        }
    }
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
/// For each chat in `hwm_map`, fetches messages newer than the last seen message
/// ID, in the order `mode` asks for. [`SyncMode::Reconcile`] overrides all HWMs
/// with a message-ID boundary computed from `now - N days`.
pub async fn sync_chats<C: TelegramClient>(
    client: &C,
    hwm_map: HashMap<i64, i64>,
    limit: i32,
    mode: SyncMode,
) -> HashMap<i64, SyncResult> {
    let mut results = HashMap::new();

    match mode {
        SyncMode::Reconcile { days } => {
            let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
            let timestamp = cutoff.timestamp() as i32;

            for &chat_id in hwm_map.keys() {
                let result = sync_single_chat_by_timestamp(client, chat_id, timestamp, limit).await;
                results.insert(chat_id, result);
            }
        }
        SyncMode::NewestFirst => {
            for (chat_id, hwm_message_id) in hwm_map {
                let result = sync_single_chat(client, chat_id, hwm_message_id, limit).await;
                results.insert(chat_id, result);
            }
        }
        SyncMode::OldestFirst => {
            for (chat_id, hwm_message_id) in hwm_map {
                let result = client
                    .get_messages_after(chat_id, hwm_message_id, limit)
                    .await;
                results.insert(chat_id, SyncResult::from(result));
            }
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

    let result = client.get_messages(chat_id, limit, until).await;
    SyncResult::from(result.map(|mut messages| {
        // Drop the boundary message itself — it was already ingested
        if hwm_message_id > 0 {
            messages.retain(|m| m.id != hwm_message_id);
        }
        messages
    }))
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

    let until_message_id = match boundary {
        BoundaryResult::BoundAt(id) => Some(id),
        BoundaryResult::None => None,
    };

    let result = client.get_messages(chat_id, limit, until_message_id).await;
    // The boundary orders by message id; the cutoff is a date. Filter to make
    // the requested window the one that is actually returned.
    SyncResult::from(result.map(|messages| {
        messages
            .into_iter()
            .filter(|m| m.timestamp >= timestamp)
            .collect()
    }))
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
            sender_is_bot: Some(false),
            text: format!("msg {id}"),
            date: "1h ago".to_string(),
            timestamp,
            is_outgoing: false,
            edit_date: None,
            content_type: Some("text".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: None,
            reply_to_message_id: None,
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst).await;
        assert_eq!(results.len(), 2);

        for result in results.values() {
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst).await;
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert_eq!(msgs.len(), 2),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[tokio::test]
    async fn sync_empty_hwm_map() {
        let client = MockClient::default();
        let results = sync_chats(&client, HashMap::new(), 20, SyncMode::NewestFirst).await;
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst).await;
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
        // Inside the 7-day reconcile window, so it survives the cutoff filter.
        let within_window = (chrono::Utc::now() - chrono::Duration::days(1)).timestamp() as i32;
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            messages: vec![make_message(1, 1, within_window)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, 99999i64); // message ID ignored when reconcile_days is set

        let results = sync_chats(&client, hwm_map, 20, SyncMode::Reconcile { days: 7 }).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[tokio::test]
    async fn sync_reconcile_days_drops_messages_before_the_cutoff() {
        // A message older than the reconcile window must not come back, even
        // though the id boundary would have admitted it.
        let before_window = (chrono::Utc::now() - chrono::Duration::days(30)).timestamp() as i32;
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            messages: vec![make_message(1, 1, before_window)],
            ..MockClient::default()
        };

        let mut hwm_map = HashMap::new();
        hwm_map.insert(1i64, 0i64);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::Reconcile { days: 7 }).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(
                msgs.is_empty(),
                "message older than the reconcile window must be dropped"
            ),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    // --- oldest-first tests ---

    fn messages_result(result: &SyncResult) -> Vec<i64> {
        match result {
            SyncResult::Messages(msgs) => msgs.iter().map(|m| m.id).collect(),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    fn call_count(counter: &std::sync::atomic::AtomicUsize) -> usize {
        counter.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn sync_oldest_first_returns_the_oldest_messages_above_the_hwm_ascending() {
        let client = MockClient {
            messages: [40, 10, 30, 20, 50]
                .into_iter()
                .map(|id| make_message(id, 1, 1000))
                .collect(),
            ..MockClient::default()
        };
        let hwm_map = HashMap::from([(1i64, 10i64)]);

        let results = sync_chats(&client, hwm_map, 2, SyncMode::OldestFirst).await;
        assert_eq!(messages_result(&results[&1]), vec![20, 30]);
        assert_eq!(call_count(&client.get_messages_after_call_count), 1);
        assert_eq!(
            call_count(&client.get_messages_call_count),
            0,
            "oldest-first must not go through the newest-first walker"
        );
    }

    #[tokio::test]
    async fn sync_oldest_first_hwm_zero_starts_at_the_oldest_message() {
        let client = MockClient {
            messages: [3, 1, 2]
                .into_iter()
                .map(|id| make_message(id, 1, 1000))
                .collect(),
            ..MockClient::default()
        };
        let hwm_map = HashMap::from([(1i64, 0i64)]);

        let results = sync_chats(&client, hwm_map, 2, SyncMode::OldestFirst).await;
        assert_eq!(messages_result(&results[&1]), vec![1, 2]);
    }

    #[tokio::test]
    async fn sync_oldest_first_reports_a_failing_chat_in_band() {
        let client = MockClient {
            inaccessible_chat_ids: vec![999],
            messages: vec![make_message(1, 1, 1000)],
            ..MockClient::default()
        };
        let hwm_map = HashMap::from([(1i64, 0i64), (999i64, 0i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst).await;
        assert_eq!(messages_result(&results[&1]), vec![1]);
        assert!(matches!(results[&999], SyncResult::Error { .. }));
    }

    #[tokio::test]
    async fn sync_default_mode_stays_newest_first() {
        let client = MockClient {
            messages: vec![make_message(20, 1, 1000), make_message(10, 1, 1000)],
            ..MockClient::default()
        };
        let hwm_map = HashMap::from([(1i64, 0i64)]);

        sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst).await;
        assert_eq!(call_count(&client.get_messages_call_count), 1);
        assert_eq!(call_count(&client.get_messages_after_call_count), 0);
    }

    #[test]
    fn sync_mode_from_request_fields() {
        assert_eq!(SyncMode::new(None, false).unwrap(), SyncMode::NewestFirst);
        assert_eq!(SyncMode::new(None, true).unwrap(), SyncMode::OldestFirst);
        assert_eq!(
            SyncMode::new(Some(7), false).unwrap(),
            SyncMode::Reconcile { days: 7 }
        );
    }

    #[test]
    fn sync_mode_refuses_oldest_first_with_reconcile_days() {
        // A reconcile sweep reads newest-first by design; silently dropping
        // either flag would hand the caller a window it did not ask for.
        let err = SyncMode::new(Some(7), true).unwrap_err().to_string();
        assert!(err.contains("oldest_first"), "{err}");
        assert!(err.contains("reconcile_days"), "{err}");
    }

    #[tokio::test]
    async fn handle_refuses_oldest_first_with_reconcile_days() {
        let client = MockClient::default();
        let req = SyncRequest {
            hwm: HashMap::from([("1".to_string(), 0i64)]),
            limit: 20,
            reconcile_days: Some(7),
            oldest_first: true,
        };
        assert!(handle(&client, req).await.is_err());
        assert_eq!(call_count(&client.get_messages_call_count), 0);
        assert_eq!(call_count(&client.get_messages_after_call_count), 0);
    }

    #[test]
    fn sync_request_without_oldest_first_is_newest_first() {
        // Wire compatibility: every existing caller omits the key.
        let req: SyncRequest = serde_json::from_value(serde_json::json!({
            "hwm": {"1": 0}, "limit": 100
        }))
        .unwrap();
        assert!(!req.oldest_first);
    }

    #[test]
    fn sync_request_carries_oldest_first() {
        let req: SyncRequest = serde_json::from_value(serde_json::json!({
            "hwm": {"1": 0}, "limit": 100, "oldest_first": true
        }))
        .unwrap();
        assert!(req.oldest_first);
        assert!(
            serde_json::to_value(&req).unwrap()["oldest_first"]
                .as_bool()
                .unwrap()
        );
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

    // --- round-trip tests (server emit -> client parse) ---
    //
    // Mirrors the wire path: `sync::handle` builds `HashMap<i64, SyncResult>`,
    // `serve::execute` serializes it via `serde_json::to_value`, and the
    // client in `main.rs` deserializes the response into
    // `HashMap<String, SyncResult>`. A bug here surfaces in production as
    // `tg serve: result parse error: data did not match any variant of
    // untagged enum SyncResult`.
    #[test]
    fn sync_result_messages_roundtrips_through_serde() {
        let original = SyncResult::Messages(vec![make_message(1, 1, 1000)]);
        let json = serde_json::to_value(&original).unwrap();
        let back: SyncResult = serde_json::from_value(json).expect("Messages should roundtrip");
        match back {
            SyncResult::Messages(msgs) => assert_eq!(msgs.len(), 1),
            SyncResult::Error { error } => panic!("got Error: {error}"),
        }
    }

    #[test]
    fn sync_result_empty_messages_roundtrips_through_serde() {
        let original = SyncResult::Messages(vec![]);
        let json = serde_json::to_value(&original).unwrap();
        let back: SyncResult =
            serde_json::from_value(json).expect("empty Messages should roundtrip");
        match back {
            SyncResult::Messages(msgs) => assert!(msgs.is_empty()),
            SyncResult::Error { error } => panic!("got Error: {error}"),
        }
    }

    #[test]
    fn sync_result_error_roundtrips_through_serde() {
        let original = SyncResult::Error {
            error: "boom".to_string(),
        };
        let json = serde_json::to_value(&original).unwrap();
        let back: SyncResult = serde_json::from_value(json).expect("Error should roundtrip");
        match back {
            SyncResult::Error { error } => assert_eq!(error, "boom"),
            SyncResult::Messages(_) => panic!("got Messages, expected Error"),
        }
    }

    #[test]
    fn full_sync_output_roundtrips_server_to_client() {
        // Server emits HashMap<i64, SyncResult>; client receives
        // HashMap<String, SyncResult>. The whole JSON value must survive.
        let mut server_side: HashMap<i64, SyncResult> = HashMap::new();
        server_side.insert(123, SyncResult::Messages(vec![make_message(1, 123, 1000)]));
        server_side.insert(456, SyncResult::Messages(vec![]));
        server_side.insert(
            999,
            SyncResult::Error {
                error: "Not found".to_string(),
            },
        );

        let json = serde_json::to_value(&server_side).unwrap();
        let client_side: HashMap<String, SyncResult> =
            serde_json::from_value(json).expect("server payload must parse client-side");

        assert_eq!(client_side.len(), 3);
        match &client_side["123"] {
            SyncResult::Messages(msgs) => assert_eq!(msgs.len(), 1),
            SyncResult::Error { error } => panic!("123 should be Messages, got Error: {error}"),
        }
        match &client_side["456"] {
            SyncResult::Messages(msgs) => assert!(msgs.is_empty()),
            SyncResult::Error { error } => panic!("456 should be Messages, got Error: {error}"),
        }
        match &client_side["999"] {
            SyncResult::Error { error } => assert_eq!(error, "Not found"),
            SyncResult::Messages(_) => panic!("999 should be Error"),
        }
    }
}
