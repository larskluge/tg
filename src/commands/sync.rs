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
    /// Report a private chat the account has deleted as
    /// [`SyncResult::ChatDeleted`] instead of an empty message list. Opt-in, so
    /// a caller that does not ask never sees the new shape.
    #[serde(default)]
    pub report_deleted: bool,
}

impl Default for SyncRequest {
    fn default() -> Self {
        Self {
            hwm: HashMap::new(),
            limit: default_sync_limit(),
            reconcile_days: None,
            oldest_first: false,
            report_deleted: false,
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
    Ok(sync_chats(client, hwm_map, req.limit, mode, req.report_deleted).await)
}

/// Per-chat sync outcome: messages, an error description, or — only when the
/// request asked for it (`report_deleted`) — the report that the account has
/// deleted this private chat.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SyncResult {
    Messages(Vec<MessageInfo>),
    Error {
        error: String,
    },
    /// Serialises as `{"chat_deleted": true}`; `chat_deleted` is always `true`.
    /// It means the chat holds no messages any more, so every message a caller
    /// stored from it is deleted too, at any age. Never inferred from an error
    /// or from an empty fetch alone: see `chat_is_deleted`.
    ChatDeleted {
        chat_deleted: bool,
    },
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
///
/// With `report_deleted`, a chat whose fetch came back empty is also asked
/// whether the account deleted it ([`chat_is_deleted`]), and answers
/// [`SyncResult::ChatDeleted`] when it did.
pub async fn sync_chats<C: TelegramClient>(
    client: &C,
    hwm_map: HashMap<i64, i64>,
    limit: i32,
    mode: SyncMode,
    report_deleted: bool,
) -> HashMap<i64, SyncResult> {
    // One clock reading for the whole sweep, so every chat is reconciled over
    // the same window.
    let now = chrono::Utc::now();

    let mut results = HashMap::with_capacity(hwm_map.len());
    for (chat_id, hwm_message_id) in hwm_map {
        let fetched = match mode {
            SyncMode::NewestFirst => sync_single_chat(client, chat_id, hwm_message_id, limit).await,
            SyncMode::OldestFirst => {
                client
                    .get_messages_after(chat_id, hwm_message_id, limit)
                    .await
            }
            SyncMode::Reconcile { days } => {
                let timestamp = (now - chrono::Duration::days(days as i64)).timestamp() as i32;
                sync_single_chat_by_timestamp(client, chat_id, timestamp, limit).await
            }
        };

        let result = match fetched {
            Ok(messages) if messages.is_empty() && report_deleted => {
                match chat_is_deleted(client, chat_id).await {
                    Ok(true) => SyncResult::ChatDeleted { chat_deleted: true },
                    Ok(false) => SyncResult::Messages(messages),
                    Err(e) => SyncResult::Error {
                        error: format!("deleted-chat check failed: {e}"),
                    },
                }
            }
            other => SyncResult::from(other),
        };
        results.insert(chat_id, result);
    }

    results
}

/// Has the account deleted this private chat?
///
/// Deleting a chat leaves it in no chat list with no last message
/// ([`crate::client::ChatPresence::looks_deleted`]). TDLib also shows that
/// state, briefly, for a chat it has just loaded from its database and whose
/// last message it has yet to load, so the state alone is not trusted: the
/// chat's history is read from its end — TDLib goes to its database and then
/// the server before it answers empty, and marks a chat empty only on a server
/// answer — and the state must still hold afterwards. An error anywhere is an
/// error, never a deletion.
async fn chat_is_deleted<C: TelegramClient>(client: &C, chat_id: i64) -> Result<bool> {
    if !client.get_chat_presence(chat_id).await?.looks_deleted() {
        return Ok(false);
    }
    if !client.get_messages(chat_id, 1, None).await?.is_empty() {
        return Ok(false);
    }
    Ok(client.get_chat_presence(chat_id).await?.looks_deleted())
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
) -> Result<Vec<MessageInfo>> {
    let until = if hwm_message_id > 0 {
        Some(hwm_message_id)
    } else {
        None
    };

    let mut messages = client.get_messages(chat_id, limit, until).await?;
    // Drop the boundary message itself — it was already ingested
    if hwm_message_id > 0 {
        messages.retain(|m| m.id != hwm_message_id);
    }
    Ok(messages)
}

/// Fetch messages newer than a timestamp for a single chat (used by --reconcile-days).
///
/// Falls back to timestamp-based boundary lookup since we don't have a message ID.
async fn sync_single_chat_by_timestamp<C: TelegramClient>(
    client: &C,
    chat_id: i64,
    timestamp: i32,
    limit: i32,
) -> Result<Vec<MessageInfo>> {
    // Warmup fetch to trigger TDLib server sync
    client.get_messages(chat_id, 1, None).await?;

    let until_message_id = match client.get_boundary_message_id(chat_id, timestamp).await? {
        BoundaryResult::BoundAt(id) => Some(id),
        BoundaryResult::None => None,
    };

    let messages = client
        .get_messages(chat_id, limit, until_message_id)
        .await?;
    // The boundary orders by message id; the cutoff is a date. Filter to make
    // the requested window the one that is actually returned.
    Ok(messages
        .into_iter()
        .filter(|m| m.timestamp >= timestamp)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ChatPresence;
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst, false).await;
        assert_eq!(results.len(), 2);

        for result in results.values() {
            match result {
                SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
                SyncResult::Error { error } => panic!("unexpected error: {error}"),
                SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst, false).await;
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
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst, false).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert_eq!(msgs.len(), 2),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
        }
    }

    #[tokio::test]
    async fn sync_empty_hwm_map() {
        let client = MockClient::default();
        let results = sync_chats(&client, HashMap::new(), 20, SyncMode::NewestFirst, false).await;
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst, false).await;
        assert_eq!(results.len(), 2);

        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
            SyncResult::Error { error } => panic!("chat 1 should succeed, got: {error}"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
        }

        match &results[&999] {
            SyncResult::Error { .. } => {} // expected
            SyncResult::Messages(_) => panic!("chat 999 should fail"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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

        let results =
            sync_chats(&client, hwm_map, 20, SyncMode::Reconcile { days: 7 }, false).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(!msgs.is_empty()),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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

        let results =
            sync_chats(&client, hwm_map, 20, SyncMode::Reconcile { days: 7 }, false).await;
        match &results[&1] {
            SyncResult::Messages(msgs) => assert!(
                msgs.is_empty(),
                "message older than the reconcile window must be dropped"
            ),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
        }
    }

    // --- oldest-first tests ---

    fn messages_result(result: &SyncResult) -> Vec<i64> {
        match result {
            SyncResult::Messages(msgs) => msgs.iter().map(|m| m.id).collect(),
            SyncResult::Error { error } => panic!("unexpected error: {error}"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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

        let results = sync_chats(&client, hwm_map, 2, SyncMode::OldestFirst, false).await;
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

        let results = sync_chats(&client, hwm_map, 2, SyncMode::OldestFirst, false).await;
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

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, false).await;
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

        sync_chats(&client, hwm_map, 20, SyncMode::NewestFirst, false).await;
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
            report_deleted: false,
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

    // --- deleted-chat tests ---

    fn presence(is_private: bool, in_chat_list: bool, has_last_message: bool) -> ChatPresence {
        ChatPresence {
            is_private,
            in_chat_list,
            has_last_message,
        }
    }

    /// The state a private chat is left in once the account deleted it.
    fn deleted() -> ChatPresence {
        presence(true, false, false)
    }

    fn listed() -> ChatPresence {
        presence(true, true, true)
    }

    fn client_with_presence(
        messages: Vec<MessageInfo>,
        answers: Vec<(i64, Vec<ChatPresence>)>,
    ) -> MockClient {
        MockClient {
            messages,
            chat_presence: std::sync::Mutex::new(answers.into_iter().collect()),
            ..MockClient::default()
        }
    }

    fn is_chat_deleted(result: &SyncResult) -> bool {
        matches!(result, SyncResult::ChatDeleted { chat_deleted: true })
    }

    #[tokio::test]
    async fn a_deleted_private_chat_is_reported_as_deleted() {
        let client = client_with_presence(vec![], vec![(1, vec![deleted()])]);
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert!(is_chat_deleted(&results[&1]), "{:?}", results[&1]);
    }

    #[tokio::test]
    async fn without_report_deleted_a_deleted_chat_stays_an_empty_list() {
        // Wire compatibility: a caller that did not ask never sees the new shape,
        // and tg does not spend a TDLib call on the question.
        let client = client_with_presence(vec![], vec![(1, vec![deleted()])]);
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, false).await;
        assert_eq!(messages_result(&results[&1]), Vec::<i64>::new());
        assert_eq!(call_count(&client.get_chat_presence_call_count), 0);
    }

    #[tokio::test]
    async fn only_the_deleted_chat_of_several_is_reported() {
        let client = client_with_presence(vec![], vec![(1, vec![deleted()]), (2, vec![listed()])]);
        let hwm_map = HashMap::from([(1i64, 42i64), (2i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert!(is_chat_deleted(&results[&1]));
        assert_eq!(messages_result(&results[&2]), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn an_empty_chat_that_is_still_in_a_chat_list_is_not_deleted() {
        // A cleared history, or a chat whose only message expired: nothing to
        // read, but the account still has the chat.
        let client = client_with_presence(vec![], vec![(1, vec![presence(true, true, false)])]);
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert_eq!(messages_result(&results[&1]), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn a_group_in_no_chat_list_is_never_reported_deleted() {
        // Leaving a group is not deleting a chat; it is out of scope here.
        let client =
            client_with_presence(vec![], vec![(-100, vec![presence(false, false, false)])]);
        let hwm_map = HashMap::from([(-100i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert_eq!(messages_result(&results[&-100]), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn a_chat_with_a_last_message_is_not_deleted() {
        let client = client_with_presence(vec![], vec![(1, vec![presence(true, false, true)])]);
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert_eq!(messages_result(&results[&1]), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn a_chat_that_returned_messages_is_never_checked() {
        let client =
            client_with_presence(vec![make_message(50, 1, 1000)], vec![(1, vec![deleted()])]);
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert_eq!(messages_result(&results[&1]), vec![50]);
        assert_eq!(call_count(&client.get_chat_presence_call_count), 0);
    }

    #[tokio::test]
    async fn the_history_from_the_end_of_the_chat_must_come_back_empty() {
        // Nothing above the cursor, and a state that looks deleted — but the
        // chat still holds a message, so it is not deleted.
        let client =
            client_with_presence(vec![make_message(42, 1, 1000)], vec![(1, vec![deleted()])]);
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert_eq!(messages_result(&results[&1]), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn the_deleted_state_must_still_hold_after_the_history_read() {
        // TDLib shows the deleted state for a chat it has only just loaded
        // from its database; reading the history is what settles it.
        let client = client_with_presence(vec![], vec![(1, vec![deleted(), listed()])]);
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert_eq!(messages_result(&results[&1]), Vec::<i64>::new());
        assert_eq!(call_count(&client.get_chat_presence_call_count), 2);
    }

    #[tokio::test]
    async fn a_failed_presence_read_is_an_error_never_a_deletion() {
        let client = MockClient {
            messages: vec![],
            presence_error_chat_ids: vec![1],
            ..MockClient::default()
        };
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        match &results[&1] {
            SyncResult::Error { error } => assert!(error.contains("Chat not found"), "{error}"),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failed_fetch_is_an_error_never_a_deletion() {
        let client = MockClient {
            messages: vec![],
            inaccessible_chat_ids: vec![1],
            chat_presence: std::sync::Mutex::new(HashMap::from([(1, vec![deleted()])])),
            ..MockClient::default()
        };
        let hwm_map = HashMap::from([(1i64, 42i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::OldestFirst, true).await;
        assert!(matches!(results[&1], SyncResult::Error { .. }));
        assert_eq!(call_count(&client.get_chat_presence_call_count), 0);
    }

    #[tokio::test]
    async fn an_empty_reconcile_window_is_not_a_deletion() {
        // The reconcile sweep's window is empty for every quiet chat; the chat's
        // older history is what shows it still exists.
        let before_window = (chrono::Utc::now() - chrono::Duration::days(30)).timestamp() as i32;
        let client = client_with_presence(
            vec![make_message(1, 1, before_window)],
            vec![(1, vec![deleted()])],
        );
        let hwm_map = HashMap::from([(1i64, 0i64)]);

        let results = sync_chats(&client, hwm_map, 20, SyncMode::Reconcile { days: 7 }, true).await;
        assert_eq!(messages_result(&results[&1]), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn every_sync_mode_reports_a_deleted_chat() {
        for mode in [
            SyncMode::NewestFirst,
            SyncMode::OldestFirst,
            SyncMode::Reconcile { days: 7 },
        ] {
            let client = client_with_presence(vec![], vec![(1, vec![deleted()])]);
            let hwm_map = HashMap::from([(1i64, 42i64)]);

            let results = sync_chats(&client, hwm_map, 20, mode, true).await;
            assert!(is_chat_deleted(&results[&1]), "{mode:?}: {:?}", results[&1]);
        }
    }

    #[tokio::test]
    async fn handle_carries_report_deleted_through() {
        let client = client_with_presence(vec![], vec![(1, vec![deleted()])]);
        let req: SyncRequest = serde_json::from_value(serde_json::json!({
            "hwm": {"1": 42}, "limit": 20, "oldest_first": true, "report_deleted": true
        }))
        .unwrap();

        let results = handle(&client, req).await.unwrap();
        assert!(is_chat_deleted(&results[&1]));
    }

    #[test]
    fn sync_request_without_report_deleted_does_not_report() {
        let req: SyncRequest = serde_json::from_value(serde_json::json!({
            "hwm": {"1": 0}, "limit": 100
        }))
        .unwrap();
        assert!(!req.report_deleted);
    }

    #[test]
    fn chat_deleted_serializes_as_an_object() {
        let json = serde_json::to_value(SyncResult::ChatDeleted { chat_deleted: true }).unwrap();
        assert_eq!(json, serde_json::json!({"chat_deleted": true}));
    }

    #[test]
    fn chat_deleted_roundtrips_server_to_client() {
        let server_side: HashMap<i64, SyncResult> = HashMap::from([
            (1, SyncResult::ChatDeleted { chat_deleted: true }),
            (2, SyncResult::Messages(vec![])),
            (
                3,
                SyncResult::Error {
                    error: "boom".to_string(),
                },
            ),
        ]);

        let json = serde_json::to_value(&server_side).unwrap();
        let client_side: HashMap<String, SyncResult> = serde_json::from_value(json).unwrap();

        assert!(is_chat_deleted(&client_side["1"]));
        assert!(matches!(&client_side["2"], SyncResult::Messages(m) if m.is_empty()));
        assert!(matches!(&client_side["3"], SyncResult::Error { error } if error == "boom"));
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
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
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
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
        }
        match &client_side["456"] {
            SyncResult::Messages(msgs) => assert!(msgs.is_empty()),
            SyncResult::Error { error } => panic!("456 should be Messages, got Error: {error}"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
        }
        match &client_side["999"] {
            SyncResult::Error { error } => assert_eq!(error, "Not found"),
            SyncResult::Messages(_) => panic!("999 should be Error"),
            SyncResult::ChatDeleted { .. } => panic!("unexpected chat_deleted"),
        }
    }
}
