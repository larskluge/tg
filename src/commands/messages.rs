use serde::{Deserialize, Serialize};

use crate::cli::MessagesArgs;
use crate::client::{BoundaryResult, TelegramClient};
use crate::error::{Result, TgError};
use crate::output::MessageInfo;

fn default_limit() -> i32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub chat: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: i32,
    #[serde(default)]
    pub since_utc: Option<String>,
    #[serde(default)]
    pub oldest_first: bool,
}

impl Default for MessagesRequest {
    fn default() -> Self {
        Self {
            name: None,
            chat: None,
            limit: default_limit(),
            since_utc: None,
            oldest_first: false,
        }
    }
}

impl From<MessagesArgs> for MessagesRequest {
    fn from(args: MessagesArgs) -> Self {
        Self {
            name: args.name,
            chat: args.chat,
            limit: args.limit,
            since_utc: args.since_utc,
            oldest_first: args.oldest_first,
        }
    }
}

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

    let since_timestamp = since_utc.map(parse_since_date).transpose()?;

    let until_message_id = if let Some(timestamp) = since_timestamp {
        // Warm up TDLib's local cache by fetching the latest message first. This
        // triggers getChatHistory(only_local=false), forcing TDLib to sync from the
        // server. Without this, getChatMessageByDate and subsequent getChatHistory
        // calls may return stale data from the local cache.
        client.get_messages(chat_id, 1, None).await?;

        match client.get_boundary_message_id(chat_id, timestamp).await? {
            BoundaryResult::BoundAt(id) => Some(id),
            BoundaryResult::None => None,
        }
    } else {
        None
    };

    // `--oldest-first` needs the whole qualifying range before it can pick the
    // oldest `limit` of it; the boundary keeps that walk bounded when there is one.
    let fetch_limit = if oldest_first { i32::MAX } else { limit };
    let mut messages = client
        .get_messages(chat_id, fetch_limit, until_message_id)
        .await?;

    // The boundary orders by message id; `--since-utc` is a claim about dates.
    // Filtering here is what actually guarantees the cutoff, and it must happen
    // before `limit` is applied or `--oldest-first` would spend its budget on
    // messages that are about to be discarded.
    if let Some(ts) = since_timestamp {
        messages.retain(|m| m.timestamp >= ts);
    }

    if oldest_first {
        messages.reverse();
        messages.truncate(limit as usize);
    }

    Ok(MessagesResult { chat_id, messages })
}

pub async fn handle<C: TelegramClient>(
    client: &C,
    req: MessagesRequest,
) -> Result<Vec<MessageInfo>> {
    let target = if let Some(id) = req.chat {
        ChatTarget::Id(id)
    } else if let Some(name) = req.name {
        ChatTarget::Name(name)
    } else {
        return Err(TgError::Other(
            "messages: either `chat` or `name` is required".to_string(),
        ));
    };
    let result = get_messages(
        client,
        target,
        req.limit,
        req.since_utc.as_deref(),
        req.oldest_first,
    )
    .await?;
    Ok(result.messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    /// A chat-1 message with the given id and Unix timestamp.
    fn make_message(id: i64, timestamp: i32) -> MessageInfo {
        MessageInfo {
            id,
            chat_id: 1,
            sender_id: Some(300),
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
        }
    }

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
        let result = get_messages(
            &client,
            ChatTarget::Name("John".to_string()),
            20,
            None,
            false,
        )
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
    async fn since_utc_future_date_returns_empty() {
        // A cutoff after the newest message: the probe lands on that newest
        // message, so the bound sits above every id in the chat.
        let cutoff = 1774483200; // 2026-03-20 00:00 UTC
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(3),
            messages: vec![make_message(2, cutoff - 60), make_message(1, cutoff - 120)],
            ..MockClient::default()
        };
        let result = get_messages(
            &client,
            ChatTarget::Id(1),
            20,
            Some("2026-03-20T00:00:00Z"),
            false,
        )
        .await
        .unwrap();
        assert!(
            result.messages.is_empty(),
            "a cutoff newer than every message must return nothing"
        );
    }

    #[tokio::test]
    async fn boundary_alone_excludes_messages_below_it() {
        // Every message here is inside the window, so the timestamp filter is a
        // no-op and the id bound is the only thing that can exclude anything.
        // This is what pins the bound itself rather than the filter behind it.
        let cutoff = 1772323200; // 2026-03-01 00:00 UTC
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(3),
            messages: vec![
                make_message(4, cutoff + 240),
                make_message(3, cutoff + 180),
                make_message(2, cutoff + 120),
                make_message(1, cutoff + 60),
            ],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 20, Some("2026-03-01"), false)
            .await
            .unwrap();
        let ids: Vec<i64> = result.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![4, 3], "only ids at or above the bound may return");
    }

    #[tokio::test]
    async fn oldest_first_with_boundary_returns_oldest_of_the_window() {
        // The production shape once the boundary lookup works: bounded fetch,
        // then the oldest `limit` of what the bound admitted.
        let cutoff = 1772323200; // 2026-03-01 00:00 UTC
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(3),
            messages: vec![
                make_message(5, cutoff + 300),
                make_message(4, cutoff + 240),
                make_message(3, cutoff + 180),
                make_message(2, cutoff - 60),
                make_message(1, cutoff - 120),
            ],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 2, Some("2026-03-01"), true)
            .await
            .unwrap();
        let ids: Vec<i64> = result.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![3, 4], "oldest two inside the bounded window");
    }

    #[tokio::test]
    async fn since_utc_looks_the_boundary_up_exactly_once() {
        // The lookup used to be retried after a 300ms sleep whenever it came back
        // None. It no longer misses by construction, and None is now a definitive
        // answer, so a second probe would be pure latency.
        let client = MockClient {
            boundary_result: BoundaryResult::None,
            ..MockClient::default()
        };
        let boundary_calls = client.get_boundary_call_count.clone();

        let _ = get_messages(&client, ChatTarget::Id(1), 20, Some("2020-01-01"), false).await;

        let count = boundary_calls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(count, 1, "expected exactly 1 boundary lookup, got {count}");
    }

    #[tokio::test]
    async fn since_utc_date_only_is_inclusive() {
        // A message sent exactly at the cutoff is inside the window, as is
        // everything newer. Boundary id=1 covers both mock messages.
        let cutoff = 1767225600; // 2026-01-01 00:00 UTC
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            messages: vec![make_message(2, cutoff + 60), make_message(1, cutoff)],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 20, Some("2026-01-01"), false)
            .await
            .unwrap();
        assert!(
            result.messages.iter().any(|m| m.id == 1),
            "message sent exactly at the cutoff should be included (inclusive)"
        );
        assert!(
            result.messages.iter().any(|m| m.id == 2),
            "newer message (id=2) should be included"
        );
    }

    #[tokio::test]
    async fn since_utc_iso8601_with_time_is_inclusive() {
        // Same inclusivity rule, expressed as a full ISO 8601 timestamp — this
        // verifies the datetime string parses and that second-level equality
        // still counts as inside the window.
        let cutoff = 1773826445; // 2026-03-18T09:34:05Z
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            messages: vec![make_message(2, cutoff + 60), make_message(1, cutoff)],
            ..MockClient::default()
        };
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
            "message at exactly the ISO 8601 cutoff should be included"
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

    #[tokio::test]
    async fn since_utc_without_boundary_filters_by_timestamp() {
        // A 404 from the boundary probe means no message predates the cutoff, so
        // the fetch runs unbounded. The timestamp filter still has to hold.
        let cutoff = 1772323200; // 2026-03-01 00:00 UTC
        let client = MockClient {
            boundary_result: BoundaryResult::None,
            messages: vec![
                make_message(10, cutoff + 3600),
                make_message(9, cutoff - 3600),
            ],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 20, Some("2026-03-01"), false)
            .await
            .unwrap();
        let ids: Vec<i64> = result.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![10], "only messages >= the cutoff may be returned");
    }

    #[tokio::test]
    async fn oldest_first_with_since_filters_before_truncating() {
        // History (newest-first): ids 4, 3 are at/after the cutoff; 2, 1 are before it.
        // With --oldest-first --limit 2 the answer is the two OLDEST messages that
        // are still at or after the cutoff — ids 3 then 4. Truncating before
        // filtering picks the two oldest of the whole history (1, 2) and then
        // filters both away, yielding nothing.
        let cutoff = 1772323200; // 2026-03-01 00:00 UTC
        let client = MockClient {
            boundary_result: BoundaryResult::None,
            messages: vec![
                make_message(4, cutoff + 7200),
                make_message(3, cutoff + 3600),
                make_message(2, cutoff - 3600),
                make_message(1, cutoff - 7200),
            ],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 2, Some("2026-03-01"), true)
            .await
            .unwrap();
        let ids: Vec<i64> = result.messages.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            vec![3, 4],
            "--oldest-first must filter by timestamp before applying the limit"
        );
    }

    #[tokio::test]
    async fn since_utc_never_returns_messages_older_than_cutoff() {
        // Even when the boundary lookup succeeds, the timestamp filter is the
        // guarantee: a message id boundary orders by id, and message dates are
        // not strictly monotonic with ids (imported history).
        let cutoff = 1772323200; // 2026-03-01 00:00 UTC
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(2),
            messages: vec![
                make_message(3, cutoff + 3600),
                make_message(2, cutoff - 3600),
            ],
            ..MockClient::default()
        };
        let result = get_messages(&client, ChatTarget::Id(1), 20, Some("2026-03-01"), false)
            .await
            .unwrap();
        let ids: Vec<i64> = result.messages.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            vec![3],
            "messages older than the cutoff must be dropped"
        );
    }

    #[tokio::test]
    async fn since_utc_with_boundary_fetches_twice() {
        let client = MockClient {
            boundary_result: BoundaryResult::BoundAt(1),
            ..MockClient::default()
        };
        let call_count = client.get_messages_call_count.clone();

        let _ = get_messages(&client, ChatTarget::Id(1), 20, Some("2020-01-01"), false).await;

        let count = call_count.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            count, 2,
            "expected 1 warm-up + 1 actual fetch when a boundary is found, got {count}"
        );
    }

    #[tokio::test]
    async fn since_utc_without_boundary_fetches_twice() {
        // An unbounded fetch costs the same two calls — the missing boundary
        // must not turn into extra round trips.
        let client = MockClient {
            boundary_result: BoundaryResult::None,
            ..MockClient::default()
        };
        let call_count = client.get_messages_call_count.clone();

        let _ = get_messages(&client, ChatTarget::Id(1), 20, Some("2020-01-01"), false).await;

        let count = call_count.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(count, 2, "expected 1 warm-up + 1 actual fetch, got {count}");
    }

    #[tokio::test]
    async fn no_since_utc_skips_the_warm_up_fetch() {
        // Without --since-utc there is no boundary lookup, so no warm-up either.
        let client = MockClient::default();
        let call_count = client.get_messages_call_count.clone();

        let _ = get_messages(&client, ChatTarget::Id(1), 20, None, false).await;

        let count = call_count.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            count, 1,
            "without --since-utc, expected exactly 1 get_messages call, got {count}"
        );
    }
}
