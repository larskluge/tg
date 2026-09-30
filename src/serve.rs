//! Shared serve-protocol primitives: socket path resolution, request/response
//! envelopes, the event frames a subscription carries, and the command
//! dispatcher. Used by both the server (`commands/serve.rs`) and the
//! client-side proxy (`serve_client.rs`).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tdlib_rs::enums::Update;

use crate::client::TelegramClient;
use crate::commands::{
    chats, download, groups, mark_read, mark_unread, messages, search, send, sync, unread, whoami,
};
use crate::credentials::tg_data_dir;
use crate::error::{Result, TgError};

/// Resolve the Unix socket path for `tg serve`.
///
/// - `TG_SERVE_SOCKET=/explicit/path` → use that path verbatim.
/// - `TG_SERVE_SOCKET=` (set but empty) → return `None` (disabled).
/// - `XDG_RUNTIME_DIR` set → `$XDG_RUNTIME_DIR/tg.sock`.
/// - Otherwise → `dirs::data_dir()/tg/serve.sock` (or `tg_data_dir()` fallback).
pub fn socket_path() -> Option<PathBuf> {
    match std::env::var_os("TG_SERVE_SOCKET") {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(PathBuf::from(v)),
        None => {
            if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
                && !dir.is_empty()
            {
                Some(PathBuf::from(dir).join("tg.sock"))
            } else {
                Some(tg_data_dir().join("serve.sock"))
            }
        }
    }
}

/// Incoming request envelope: `{"id": ..., "cmd": ..., "args": ...}`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RequestEnvelope {
    /// Opaque correlation token, echoed back on the matching response.
    #[serde(default)]
    pub id: serde_json::Value,
    pub cmd: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Outgoing response envelope. Either `{"ok": true, "result": ...}` or
/// `{"ok": false, "error": "..."}`, with `id` always echoed.
///
/// One failure carries BOTH: a media `send` that put some elements in the
/// recipient's chat and then could not finish answers `ok: false` with the
/// error *and* a `result` holding the per-element record (see
/// [`ResponseEnvelope::err_with_result`]). `ok` keeps its meaning — the
/// request did not do what was asked — so a caller that reads only `ok` is
/// never told a failure succeeded; what it gains is the ability to learn which
/// elements arrived instead of assuming none did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub id: serde_json::Value,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

impl ResponseEnvelope {
    pub fn ok(id: serde_json::Value, value: serde_json::Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(value),
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }

    /// A failure that still has something to report: the request did not do
    /// what was asked, and `result` says what it nonetheless did.
    pub fn err_with_result(
        id: serde_json::Value,
        message: impl Into<String>,
        value: serde_json::Value,
    ) -> Self {
        Self {
            id,
            ok: false,
            result: Some(value),
            error: Some(message.into()),
        }
    }
}

/// The `cmd` that turns a connection into a subscription. After its ack the
/// connection carries only [`StreamEvent`] frames, until the peer closes it.
pub const SUBSCRIBE_CMD: &str = "subscribe";

/// How often a subscription writes a `heartbeat` frame. The heartbeat is what
/// finds a peer that went away without closing: its write fails, and the
/// subscription ends.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// The reply to `subscribe`, written once the subscriber is already attached
/// to the update channel — so no event after the ack can be missed.
pub fn subscribe_ack(id: serde_json::Value) -> ResponseEnvelope {
    ResponseEnvelope::ok(id, serde_json::json!({"subscribed": true}))
}

/// An unsolicited frame on a subscribed connection, in the shape the protocol
/// reserved for them: `{"event": "<name>", "data": {...}}`, one per line.
///
/// A frame is a doorbell, not a record: it names a chat and a message and
/// carries no content. A consumer that wants the message fetches it (`sync`),
/// so a frame it missed costs latency, never data — which is why `lagged`
/// reports only how many were skipped.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum StreamEvent {
    /// `updateNewMessage`. `message_id` can be the temporary id of a message
    /// this account is still sending; the confirmed id follows as
    /// `chat_last_message`.
    NewMessage { chat_id: i64, message_id: i64 },
    /// `updateMessageContent` or `updateMessageEdited`. One edit usually
    /// raises both, so a consumer sees two frames for it.
    MessageEdited { chat_id: i64, message_id: i64 },
    /// `updateDeleteMessages` with `is_permanent && !from_cache` only: the
    /// other combinations describe messages that still exist.
    MessagesDeleted { chat_id: i64, message_ids: Vec<i64> },
    /// `updateChatLastMessage`. `message_id` is `null` when TDLib no longer
    /// knows the chat's last message, and while it does not, new messages can
    /// arrive with no `updateNewMessage` — so this frame rings for them.
    ChatLastMessage {
        chat_id: i64,
        message_id: Option<i64>,
    },
    /// The subscriber fell behind the update channel and `skipped` updates
    /// were dropped for it. Any of them may have been an event.
    Lagged { skipped: u64 },
    /// Written every [`HEARTBEAT_INTERVAL`], so a peer that vanished is found
    /// by the failed write.
    Heartbeat {},
}

impl StreamEvent {
    /// The event a TDLib update rings, or `None` for the updates a subscriber
    /// is not told about.
    pub fn from_update(update: &Update) -> Option<Self> {
        match update {
            Update::NewMessage(u) => Some(Self::NewMessage {
                chat_id: u.message.chat_id,
                message_id: u.message.id,
            }),
            Update::MessageContent(u) => Some(Self::MessageEdited {
                chat_id: u.chat_id,
                message_id: u.message_id,
            }),
            Update::MessageEdited(u) => Some(Self::MessageEdited {
                chat_id: u.chat_id,
                message_id: u.message_id,
            }),
            Update::DeleteMessages(u) if u.is_permanent && !u.from_cache => {
                Some(Self::MessagesDeleted {
                    chat_id: u.chat_id,
                    message_ids: u.message_ids.clone(),
                })
            }
            Update::ChatLastMessage(u) => Some(Self::ChatLastMessage {
                chat_id: u.chat_id,
                message_id: u.last_message.as_ref().map(|m| m.id),
            }),
            _ => None,
        }
    }

    /// The frame as it goes on the wire: one line of JSON, `\n`-terminated.
    pub fn to_line(&self) -> Result<String> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        Ok(line)
    }
}

/// Turn a handler error into a response, attaching the structured record when
/// the error carries one.
fn error_response(id: serde_json::Value, e: TgError) -> ResponseEnvelope {
    let message = e.to_string();
    match e.partial_send() {
        None => ResponseEnvelope::err(id, message),
        Some(partial) => match serde_json::to_value(partial) {
            Ok(value) => ResponseEnvelope::err_with_result(id, message, value),
            // Unreachable for a plain data struct, but a record that cannot be
            // serialised must not vanish: the caller would read the failure as
            // "nothing was delivered" and resend what is already delivered.
            Err(serialise) => ResponseEnvelope::err(
                id,
                format!(
                    "{message} (and the per-element record could not be serialised: {serialise} — do NOT resend blindly; check the chat)"
                ),
            ),
        },
    }
}

/// Dispatch a single request against a TelegramClient. Always returns a
/// response (errors are reported in-band).
pub async fn dispatch<C: TelegramClient>(client: &C, env: RequestEnvelope) -> ResponseEnvelope {
    let id = env.id.clone();
    let result: Result<serde_json::Value> = match env.cmd.as_str() {
        "whoami" => execute(env.args, |r| whoami::handle(client, r)).await,
        "chats" => execute(env.args, |r| chats::handle(client, r)).await,
        "groups" => execute(env.args, |r| groups::handle(client, r)).await,
        "unread" => execute(env.args, |r| unread::handle(client, r)).await,
        "search" => execute(env.args, |r| search::handle(client, r)).await,
        "messages" => execute(env.args, |r| messages::handle(client, r)).await,
        "send" => execute(env.args, |r| send::handle(client, r)).await,
        "download" => execute(env.args, |r| download::handle(client, r)).await,
        "mark_read" => execute(env.args, |r| mark_read::handle(client, r)).await,
        "mark_unread" => execute(env.args, |r| mark_unread::handle(client, r)).await,
        "sync" => execute(env.args, |r| sync::handle(client, r)).await,
        SUBSCRIBE_CMD => Err(TgError::Other(
            "subscribe turns the connection into an event stream, so only `tg serve`'s connection loop can answer it"
                .to_string(),
        )),
        "auth" | "auth_bot" | "auth_status" => Err(TgError::Other(
            "auth is not available over `tg serve`; stop the serve process and run `tg auth` directly"
                .to_string(),
        )),
        other => Err(TgError::Other(format!("unknown command: {other}"))),
    };

    match result {
        Ok(v) => ResponseEnvelope::ok(id, v),
        Err(e) => error_response(id, e),
    }
}

async fn execute<R, T, Fut>(
    args: serde_json::Value,
    handler: impl FnOnce(R) -> Fut,
) -> Result<serde_json::Value>
where
    R: serde::de::DeserializeOwned,
    T: serde::Serialize,
    Fut: std::future::Future<Output = Result<T>>,
{
    // Treat `null` and missing `args` as an empty object so commands whose
    // fields all have `#[serde(default)]` can be invoked with no args while
    // commands with required fields still error if those are missing.
    let args = if args.is_null() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        args
    };
    let req: R =
        serde_json::from_value(args).map_err(|e| TgError::Other(format!("invalid args: {e}")))?;
    let result = handler(req).await?;
    serde_json::to_value(&result).map_err(|e| TgError::Other(format!("serialization error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;
    use serde_json::json;
    use tdlib_rs::enums::Update;

    fn req(id: &str, cmd: &str, args: serde_json::Value) -> RequestEnvelope {
        RequestEnvelope {
            id: json!(id),
            cmd: cmd.to_string(),
            args,
        }
    }

    #[tokio::test]
    async fn dispatch_whoami_returns_user() {
        let client = MockClient::default();
        let res = dispatch(&client, req("1", "whoami", serde_json::Value::Null)).await;
        assert!(res.ok);
        assert_eq!(res.id, json!("1"));
        let r = res.result.unwrap();
        assert_eq!(r["id"], 42);
        assert_eq!(r["first_name"], "John");
    }

    #[tokio::test]
    async fn dispatch_chats_uses_limit_default() {
        let client = MockClient::default();
        let res = dispatch(&client, req("2", "chats", json!({}))).await;
        assert!(res.ok);
        let arr = res.result.unwrap();
        assert!(arr.is_array());
    }

    #[tokio::test]
    async fn dispatch_unknown_command_returns_error() {
        let client = MockClient::default();
        let res = dispatch(&client, req("3", "nope", serde_json::Value::Null)).await;
        assert!(!res.ok);
        let err = res.error.unwrap();
        assert!(err.contains("unknown command"));
        assert!(err.contains("nope"));
    }

    #[tokio::test]
    async fn dispatch_auth_is_refused() {
        let client = MockClient::default();
        let res = dispatch(&client, req("4", "auth", serde_json::Value::Null)).await;
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("auth is not available"));
    }

    #[tokio::test]
    async fn dispatch_handler_error_returns_in_band() {
        let client = MockClient::default();
        // mark_read without id or name should fail in the handler
        let res = dispatch(&client, req("5", "mark_read", json!({}))).await;
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("either `id` or `name`"));
    }

    #[tokio::test]
    async fn dispatch_invalid_args_returns_error_keeping_id() {
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req("6", "messages", json!({"limit": "not-a-number"})),
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.id, json!("6"));
        assert!(res.error.unwrap().contains("invalid args"));
    }

    #[tokio::test]
    async fn dispatch_send_returns_result_shape() {
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req("7", "send", json!({"message": "hi", "id": 42})),
        )
        .await;
        assert!(res.ok);
        let r = res.result.unwrap();
        assert_eq!(r["chat_id"], 42);
    }

    #[tokio::test]
    async fn dispatch_send_reports_per_element_delivery_for_a_media_send() {
        // A media send's result carries the record of every element Telegram
        // confirmed, keyed by its index in the request's `files` array.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a.pdf");
        std::fs::write(&path, b"bytes").unwrap();

        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "7p",
                "send",
                json!({
                    "message": "two files",
                    "id": 42,
                    "files": [
                        {"path": path.to_string_lossy()},
                        {"path": path.to_string_lossy()},
                    ]
                }),
            ),
        )
        .await;
        assert!(res.ok, "{:?}", res.error);
        let r = res.result.unwrap();
        assert_eq!(r["elements"]["delivered"][0]["index"], 0);
        assert_eq!(r["elements"]["delivered"][1]["index"], 1);
        // The id a caller records for the send is the first element's, the one
        // carrying the caption.
        assert_eq!(r["message_id"], r["elements"]["delivered"][0]["message_id"]);
    }

    #[tokio::test]
    async fn dispatch_send_omits_the_element_record_for_a_text_send() {
        // Back-compat, on the response side: a text send's result is exactly
        // the two keys it has always been.
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req("7q", "send", json!({"message": "hi", "id": 42})),
        )
        .await;
        assert!(res.ok, "{:?}", res.error);
        assert_eq!(
            res.result.unwrap(),
            json!({"message_id": 12345, "chat_id": 42})
        );
    }

    #[test]
    fn a_partial_send_failure_answers_with_both_error_and_result() {
        // The protocol's one `ok:false` that still carries data. Without it a
        // caller has nowhere to read what arrived, reads the failure as
        // "nothing arrived", and resends files already in the recipient's chat.
        let err = TgError::PartialSend {
            message: "send: 1 of 2 file(s) were not delivered".to_string(),
            partial: Box::new(crate::output::PartialSendResult {
                chat_id: 42,
                elements: crate::output::MediaElements {
                    delivered: vec![crate::output::DeliveredElement {
                        index: 0,
                        message_id: 900,
                    }],
                    failed: vec![crate::output::FailedElement {
                        index: 1,
                        error: "PHOTO_INVALID_DIMENSIONS".to_string(),
                    }],
                    unconfirmed: vec![],
                },
            }),
        };
        let res = error_response(json!("1"), err);
        assert!(!res.ok, "a partial delivery is not a success");
        assert!(res.error.unwrap().contains("1 of 2"));
        let r = res.result.expect("a partial failure must carry its record");
        assert_eq!(
            r["elements"]["delivered"],
            json!([{"index": 0, "message_id": 900}])
        );
        assert_eq!(r["elements"]["failed"][0]["index"], 1);
        assert_eq!(r["chat_id"], 42);
    }

    #[test]
    fn an_ordinary_failure_carries_no_result() {
        // `result` on a failure means "this is what it nonetheless did". An
        // error that did nothing must not grow one, or its presence stops
        // meaning anything.
        let res = error_response(json!("1"), TgError::Other("nope".to_string()));
        assert!(!res.ok);
        assert!(res.result.is_none());
        assert_eq!(res.error.unwrap(), "nope");
    }

    #[tokio::test]
    async fn dispatch_send_with_parse_mode_succeeds() {
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "8",
                "send",
                json!({"message": "hi", "id": 42, "parse_mode": "HTML"}),
            ),
        )
        .await;
        assert!(res.ok, "{:?}", res.error);
        assert_eq!(res.result.unwrap()["chat_id"], 42);
    }

    #[tokio::test]
    async fn dispatch_send_carries_files_through_the_envelope() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("Q3 report.pdf");
        std::fs::write(&path, b"bytes").unwrap();

        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "7f",
                "send",
                json!({
                    "message": "here it is",
                    "id": 42,
                    "files": [{"path": path.to_string_lossy(), "kind": "file"}]
                }),
            ),
        )
        .await;
        assert!(res.ok, "{:?}", res.error);
        assert_eq!(res.result.unwrap()["chat_id"], 42);

        let media = client.media_sent.lock().unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].1, "here it is");
        assert_eq!(media[0].3[0].display_name(), "Q3 report.pdf");
    }

    #[tokio::test]
    async fn dispatch_send_refuses_a_bad_file_in_band() {
        // A malformed attachment set comes back as a normal error response, so
        // the caller can retry; nothing is sent.
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "7g",
                "send",
                json!({"message": "hi", "id": 42, "files": [{"path": "relative.pdf"}]}),
            ),
        )
        .await;
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("is not absolute"));
        assert!(client.media_sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_send_rejects_an_unknown_file_field() {
        // `files[]` is as closed as `send` itself: no TDLib input type carries a
        // filename or a MIME type, so a caller that thinks it passed one must be
        // told rather than have it silently dropped.
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "7h",
                "send",
                json!({
                    "message": "hi",
                    "id": 42,
                    "files": [{"path": "/tmp/a.pdf", "filename": "b.pdf"}]
                }),
            ),
        )
        .await;
        assert!(!res.ok);
        let err = res.error.unwrap();
        assert!(err.contains("unknown field `filename`"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_send_rejects_unknown_arg() {
        // `send` is a closed set: an unsupported arg is refused in-band instead
        // of being dropped while `tg` still answers ok.
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "9",
                "send",
                json!({"message": "hi", "id": 42, "as": "@bot"}),
            ),
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.id, json!("9"));
        let err = res.error.unwrap();
        assert!(err.contains("invalid args"), "{err}");
        assert!(err.contains("unknown field `as`"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_send_refuses_a_reply_target_the_chat_does_not_hold() {
        // Over the socket, the same-chat check answers in-band and sends nothing:
        // the mock holds message 1 in chat 1 only, and 1 << 20 names nothing.
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "10",
                "send",
                json!({"message": "hi", "id": 1, "reply_to": 1_i64 << 20}),
            ),
        )
        .await;
        assert!(!res.ok);
        let err = res.error.unwrap();
        assert!(err.contains("not accessible in chat 1"), "{err}");
        assert!(client.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_send_rejects_bad_parse_mode() {
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "10",
                "send",
                json!({"message": "hi", "id": 42, "parse_mode": "markdown"}),
            ),
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.id, json!("10"));
        let err = res.error.unwrap();
        assert!(err.contains("invalid parse_mode"), "{err}");
        assert!(err.contains("HTML"), "{err}");
        assert!(err.contains("MarkdownV2"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_sync_oldest_first_returns_ascending_messages() {
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "12",
                "sync",
                json!({"hwm": {"1": 0}, "limit": 1, "oldest_first": true}),
            ),
        )
        .await;
        assert!(res.ok, "{:?}", res.error);
        let chat = res.result.unwrap()["1"].clone();
        let ids: Vec<i64> = chat
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_i64().unwrap())
            .collect();
        let oldest = client.messages.iter().map(|m| m.id).min().unwrap();
        assert_eq!(ids, vec![oldest]);
    }

    #[tokio::test]
    async fn dispatch_sync_refuses_oldest_first_with_reconcile_days() {
        let client = MockClient::default();
        let res = dispatch(
            &client,
            req(
                "13",
                "sync",
                json!({"hwm": {"1": 0}, "oldest_first": true, "reconcile_days": 7}),
            ),
        )
        .await;
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("oldest_first"));
    }

    #[tokio::test]
    async fn dispatch_other_commands_still_ignore_unknown_args() {
        // Only `send` is strict. `whoami` in particular backs the container's
        // health check, so tightening the rest has to be a conscious act.
        let client = MockClient::default();
        let res = dispatch(&client, req("11", "chats", json!({"limit": 5, "bogus": 1}))).await;
        assert!(res.ok, "{:?}", res.error);
    }

    /// A TDLib update exactly as it arrives on the wire, so each case below is
    /// read from the JSON TDLib sends rather than from a hand-built struct.
    fn update(v: serde_json::Value) -> Update {
        serde_json::from_value(v).expect("fixture is a valid TDLib update")
    }

    /// The fields of a TDLib `message` that carry no default, around the two a
    /// stream event reads.
    fn tdlib_message(chat_id: i64, id: i64) -> serde_json::Value {
        json!({
            "@type": "message",
            "id": id,
            "sender_id": {"@type": "messageSenderUser", "user_id": 7},
            "chat_id": chat_id,
            "is_outgoing": false,
            "is_pinned": false,
            "is_from_offline": false,
            "can_be_saved": true,
            "has_timestamped_media": false,
            "is_channel_post": false,
            "is_paid_star_suggested_post": false,
            "is_paid_ton_suggested_post": false,
            "contains_unread_mention": false,
            "date": 1_759_190_400,
            "edit_date": 0,
            "unread_reactions": [],
            "self_destruct_in": 0.0,
            "auto_delete_in": 0.0,
            "via_bot_user_id": 0,
            "sender_business_bot_user_id": 0,
            "sender_boost_count": 0,
            "paid_message_star_count": 0,
            "author_signature": "",
            "media_album_id": "0",
            "effect_id": "0",
            "summary_language_code": "",
            "content": {
                "@type": "messageText",
                "text": {"@type": "formattedText", "text": "hi", "entities": []}
            }
        })
    }

    const CHAT: i64 = -1_001_666_847_309;

    #[test]
    fn a_new_message_rings_new_message() {
        let u = update(json!({
            "@type": "updateNewMessage",
            "message": tdlib_message(CHAT, 89_508_544_512_i64),
        }));
        assert_eq!(
            StreamEvent::from_update(&u),
            Some(StreamEvent::NewMessage {
                chat_id: CHAT,
                message_id: 89_508_544_512,
            })
        );
    }

    #[test]
    fn a_content_change_rings_message_edited() {
        let u = update(json!({
            "@type": "updateMessageContent",
            "chat_id": CHAT,
            "message_id": 42_i64 << 20,
            "new_content": {
                "@type": "messageText",
                "text": {"@type": "formattedText", "text": "edited", "entities": []}
            },
        }));
        assert_eq!(
            StreamEvent::from_update(&u),
            Some(StreamEvent::MessageEdited {
                chat_id: CHAT,
                message_id: 42 << 20,
            })
        );
    }

    #[test]
    fn an_edit_rings_message_edited() {
        let u = update(json!({
            "@type": "updateMessageEdited",
            "chat_id": CHAT,
            "message_id": 43_i64 << 20,
            "edit_date": 1_759_190_500,
        }));
        assert_eq!(
            StreamEvent::from_update(&u),
            Some(StreamEvent::MessageEdited {
                chat_id: CHAT,
                message_id: 43 << 20,
            })
        );
    }

    fn delete(is_permanent: bool, from_cache: bool) -> Update {
        update(json!({
            "@type": "updateDeleteMessages",
            "chat_id": CHAT,
            "message_ids": [1_i64 << 20, 2_i64 << 20],
            "is_permanent": is_permanent,
            "from_cache": from_cache,
        }))
    }

    #[test]
    fn a_permanent_deletion_rings_messages_deleted() {
        assert_eq!(
            StreamEvent::from_update(&delete(true, false)),
            Some(StreamEvent::MessagesDeleted {
                chat_id: CHAT,
                message_ids: vec![1 << 20, 2 << 20],
            })
        );
    }

    #[test]
    fn a_deletion_that_is_not_permanent_or_only_from_the_cache_is_silent() {
        // A message that merely became inaccessible, or that TDLib only evicted
        // from its cache, still exists: reporting it would tell a consumer to
        // mark live messages deleted.
        assert_eq!(StreamEvent::from_update(&delete(false, false)), None);
        assert_eq!(StreamEvent::from_update(&delete(true, true)), None);
        assert_eq!(StreamEvent::from_update(&delete(false, true)), None);
    }

    #[test]
    fn a_last_message_change_rings_chat_last_message() {
        let u = update(json!({
            "@type": "updateChatLastMessage",
            "chat_id": CHAT,
            "last_message": tdlib_message(CHAT, 99_i64 << 20),
            "positions": [],
        }));
        assert_eq!(
            StreamEvent::from_update(&u),
            Some(StreamEvent::ChatLastMessage {
                chat_id: CHAT,
                message_id: Some(99 << 20),
            })
        );
    }

    #[test]
    fn an_unknown_last_message_rings_chat_last_message_with_no_id() {
        // TDLib: while the last message is unknown, new messages can arrive
        // without an updateNewMessage — so this one must still ring.
        let u = update(json!({
            "@type": "updateChatLastMessage",
            "chat_id": CHAT,
            "positions": [],
        }));
        assert_eq!(
            StreamEvent::from_update(&u),
            Some(StreamEvent::ChatLastMessage {
                chat_id: CHAT,
                message_id: None,
            })
        );
    }

    #[test]
    fn unrelated_updates_ring_nothing() {
        for u in [
            json!({"@type": "updateConnectionState", "state": {"@type": "connectionStateReady"}}),
            json!({"@type": "updateChatTitle", "chat_id": CHAT, "title": "x"}),
            json!({
                "@type": "updateChatReadInbox",
                "chat_id": CHAT,
                "last_read_inbox_message_id": 1_i64 << 20,
                "unread_count": 0,
            }),
        ] {
            assert_eq!(StreamEvent::from_update(&update(u.clone())), None, "{u}");
        }
    }

    /// The exact bytes a subscriber reads, parsed back so key order does not
    /// matter but every key and value does.
    fn frame(event: &StreamEvent) -> serde_json::Value {
        let line = event.to_line().unwrap();
        assert!(line.ends_with('\n'), "a frame is one line: {line:?}");
        assert_eq!(
            line.matches('\n').count(),
            1,
            "a frame is one line: {line:?}"
        );
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn event_frames_have_the_documented_shapes() {
        assert_eq!(
            frame(&StreamEvent::NewMessage {
                chat_id: CHAT,
                message_id: 5 << 20,
            }),
            json!({"event": "new_message", "data": {"chat_id": CHAT, "message_id": 5_i64 << 20}})
        );
        assert_eq!(
            frame(&StreamEvent::MessageEdited {
                chat_id: CHAT,
                message_id: 5 << 20,
            }),
            json!({"event": "message_edited", "data": {"chat_id": CHAT, "message_id": 5_i64 << 20}})
        );
        assert_eq!(
            frame(&StreamEvent::MessagesDeleted {
                chat_id: CHAT,
                message_ids: vec![1 << 20, 2 << 20],
            }),
            json!({"event": "messages_deleted", "data": {"chat_id": CHAT, "message_ids": [1_i64 << 20, 2_i64 << 20]}})
        );
        assert_eq!(
            frame(&StreamEvent::ChatLastMessage {
                chat_id: CHAT,
                message_id: Some(5 << 20),
            }),
            json!({"event": "chat_last_message", "data": {"chat_id": CHAT, "message_id": 5_i64 << 20}})
        );
        assert_eq!(
            frame(&StreamEvent::Lagged { skipped: 17 }),
            json!({"event": "lagged", "data": {"skipped": 17}})
        );
        assert_eq!(
            frame(&StreamEvent::Heartbeat {}),
            json!({"event": "heartbeat", "data": {}})
        );
    }

    #[test]
    fn an_unknown_last_message_is_an_explicit_null() {
        // `message_id: null` is part of the contract, not an absent key: a
        // consumer reads `data.message_id` on every chat_last_message.
        let line = StreamEvent::ChatLastMessage {
            chat_id: 1,
            message_id: None,
        }
        .to_line()
        .unwrap();
        assert_eq!(
            line,
            "{\"event\":\"chat_last_message\",\"data\":{\"chat_id\":1,\"message_id\":null}}\n"
        );
    }

    #[test]
    fn the_subscribe_ack_has_the_documented_shape() {
        assert_eq!(
            serde_json::to_string(&subscribe_ack(json!("1"))).unwrap(),
            r#"{"id":"1","ok":true,"result":{"subscribed":true}}"#
        );
    }

    #[tokio::test]
    async fn dispatch_refuses_subscribe_it_cannot_serve() {
        // `subscribe` turns a connection into a stream, which only the
        // connection loop can do. Reaching the dispatcher means a caller
        // bypassed that loop, and "unknown command" would be a lie.
        let client = MockClient::default();
        let res = dispatch(&client, req("s", "subscribe", json!({}))).await;
        assert!(!res.ok);
        let err = res.error.unwrap();
        assert!(!err.contains("unknown command"), "{err}");
        assert!(err.contains("connection"), "{err}");
    }

    // Env-var-mutating tests are gathered into one to avoid cross-test races on
    // the shared process environment.
    #[test]
    fn socket_path_respects_env_variants() {
        let prev = std::env::var_os("TG_SERVE_SOCKET");

        // Explicit path
        unsafe { std::env::set_var("TG_SERVE_SOCKET", "/tmp/tg-explicit.sock") };
        assert_eq!(
            socket_path().unwrap(),
            PathBuf::from("/tmp/tg-explicit.sock")
        );

        // Empty means disabled
        unsafe { std::env::set_var("TG_SERVE_SOCKET", "") };
        assert!(socket_path().is_none());

        // Restore prior state
        match prev {
            Some(v) => unsafe { std::env::set_var("TG_SERVE_SOCKET", v) },
            None => unsafe { std::env::remove_var("TG_SERVE_SOCKET") },
        }
    }
}
