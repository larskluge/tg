//! Shared serve-protocol primitives: socket path resolution, request/response
//! envelopes, and the command dispatcher. Used by both the server
//! (`commands/serve.rs`) and the client-side proxy (`serve_client.rs`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    async fn dispatch_other_commands_still_ignore_unknown_args() {
        // Only `send` is strict. `whoami` in particular backs the container's
        // health check, so tightening the rest has to be a conscious act.
        let client = MockClient::default();
        let res = dispatch(&client, req("11", "chats", json!({"limit": 5, "bogus": 1}))).await;
        assert!(res.ok, "{:?}", res.error);
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
