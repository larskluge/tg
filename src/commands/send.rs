use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::cli::SendArgs;
use crate::client::TelegramClient;
use crate::error::{Result, TgError};
use crate::output::SendResult;
use crate::parse_mode::ParseMode;

pub enum SendTarget {
    Id(i64),
    Name(String),
    Username(String),
    Group(String),
}

/// Wire mirror of the `send` socket args. `deny_unknown_fields` is deliberate
/// and unique to this struct among the serve requests: an unsupported arg here
/// (e.g. `as`) used to be dropped silently while `tg` still answered `ok:true`,
/// which for a recipient or identity field means a wrong message delivered with
/// no signal anywhere. A loud refusal is retryable; a silent drop is not.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendRequest {
    pub message: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// `HTML` or `MarkdownV2`; absent means plain text. Kept as a string so this
    /// struct stays a transparent wire mirror and the error text stays ours —
    /// validation happens in [`handle`].
    #[serde(default)]
    pub parse_mode: Option<String>,
}

/// Convert clap args to a `SendRequest`. Panics if `--as <bot>` is set, because
/// bot sends use the HTTP API path and never reach this code. Also expects the
/// message to have been resolved (from `--message` or stdin) in `main::run`.
impl From<SendArgs> for SendRequest {
    fn from(args: SendArgs) -> Self {
        debug_assert!(
            args.send_as.is_none(),
            "bot sends (--as) must be routed before SendRequest"
        );
        Self {
            message: args
                .message
                .expect("send message must be resolved before SendRequest"),
            name: args.name,
            id: args.id,
            to: args.to,
            group: args.group,
            parse_mode: args.parse_mode,
        }
    }
}

/// Resolve the message body to send: use `--message`/`-m` if provided, otherwise
/// read it from stdin. Errors if no `--message` is given and stdin is an
/// interactive terminal (nothing piped) or contains only whitespace.
pub fn resolve_message(message: Option<String>) -> Result<String> {
    use std::io::IsTerminal;
    let stdin = std::io::stdin();
    let is_terminal = stdin.is_terminal();
    resolve_message_from(message, is_terminal, stdin.lock())
}

/// Testable core of [`resolve_message`]: takes the terminal flag and reader
/// explicitly so stdin handling can be exercised without a real terminal.
fn resolve_message_from<R: Read>(
    message: Option<String>,
    stdin_is_terminal: bool,
    mut reader: R,
) -> Result<String> {
    if let Some(message) = message {
        return Ok(message);
    }

    if stdin_is_terminal {
        return Err(TgError::Other(
            "send: no message provided (pass --message/-m or pipe text via stdin)".to_string(),
        ));
    }

    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;

    // Strip the trailing newline(s) that pipes/`echo` append, but keep internal
    // newlines and any other trailing whitespace the user intended.
    let trimmed = buf.trim_end_matches(['\n', '\r']);
    if trimmed.trim().is_empty() {
        return Err(TgError::Other(
            "send: empty message read from stdin".to_string(),
        ));
    }

    Ok(trimmed.to_string())
}

pub async fn send_message<C: TelegramClient>(
    client: &C,
    target: SendTarget,
    message: &str,
    parse_mode: Option<ParseMode>,
) -> Result<SendResult> {
    let chat_id = match target {
        SendTarget::Id(id) => id,
        SendTarget::Name(name) => client.find_chat_by_name(&name).await?,
        SendTarget::Username(username) => client.find_chat_by_username(&username).await?,
        SendTarget::Group(name) => client.find_group_by_name(&name).await?,
    };

    client.send_message(chat_id, message, parse_mode).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: SendRequest) -> Result<SendResult> {
    // Validate the request shape before the target ladder: that ladder issues
    // TDLib contact and public-chat searches, and a malformed request must never
    // cost a round trip. This ordering is also what lets a probe against a live
    // daemon tell an upgraded `tg` from an un-upgraded one — with the ladder
    // first, a recipient-less probe returns the recipient error and proves
    // nothing. Pinned by `handle_validates_parse_mode_before_resolving_target`.
    let parse_mode = req
        .parse_mode
        .as_deref()
        .map(ParseMode::parse)
        .transpose()?;

    let target = if let Some(ref to) = req.to {
        if let Ok(id) = to.parse::<i64>() {
            SendTarget::Id(id)
        } else if let Some(username) = to.strip_prefix('@') {
            SendTarget::Username(username.to_string())
        } else {
            SendTarget::Name(to.clone())
        }
    } else if let Some(id) = req.id {
        SendTarget::Id(id)
    } else if let Some(group) = req.group {
        SendTarget::Group(group)
    } else if let Some(name) = req.name {
        SendTarget::Name(name)
    } else {
        return Err(TgError::Other(
            "send: one of `id`, `to`, `group`, or `name` is required".to_string(),
        ));
    };

    send_message(client, target, &req.message, parse_mode).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::TelegramClient;
    use crate::client::mock::MockClient;
    use crate::error::TgError;
    use serde_json::json;

    #[tokio::test]
    async fn send_by_id() {
        let client = MockClient::default();
        let result = send_message(&client, SendTarget::Id(123), "Hello", None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().chat_id, 123);
    }

    #[tokio::test]
    async fn send_by_name() {
        let client = MockClient::default();
        let result =
            send_message(&client, SendTarget::Name("John".to_string()), "Hello", None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_by_group() {
        let client = MockClient::default();
        let result = send_message(
            &client,
            SendTarget::Group("Family".to_string()),
            "Hello",
            None,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_to_unknown_contact() {
        let client = MockClient::default();
        let result = send_message(
            &client,
            SendTarget::Name("Unknown".to_string()),
            "Hello",
            None,
        )
        .await;
        assert!(matches!(result, Err(TgError::ContactNotFound(_))));
    }

    #[tokio::test]
    async fn find_chat_by_username_found() {
        let client = MockClient::default();
        // "johndoe" is in mock contacts with username
        let result = client.find_chat_by_username("johndoe").await;
        assert_eq!(result.unwrap(), 1);
    }

    #[tokio::test]
    async fn find_chat_by_username_not_found() {
        let client = MockClient::default();
        let result = client.find_chat_by_username("nonexistent").await;
        assert!(matches!(result, Err(TgError::ContactNotFound(_))));
    }

    #[tokio::test]
    async fn find_chat_by_username_case_insensitive() {
        let client = MockClient::default();
        let result = client.find_chat_by_username("JohnDoe").await;
        assert_eq!(result.unwrap(), 1);
    }

    #[tokio::test]
    async fn handle_by_id() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.chat_id, 123);
    }

    #[tokio::test]
    async fn handle_by_name() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            name: Some("John".to_string()),
            ..Default::default()
        };
        handle(&client, req).await.unwrap();
    }

    #[tokio::test]
    async fn handle_to_numeric_string_routes_to_id() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            to: Some("123".to_string()),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.chat_id, 123);
    }

    #[tokio::test]
    async fn handle_to_at_username_resolves_by_username() {
        // `@handle` must resolve via username lookup (search_public_chat), not a
        // display-name contact search. Mock contact id 1 has username "johndoe"
        // but display name "John Doe", so a name search for "johndoe" would miss.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            to: Some("@johndoe".to_string()),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.chat_id, 1);
    }

    #[tokio::test]
    async fn handle_to_plain_name_uses_name_search() {
        // A `--to` value without `@` and not numeric is a display name.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            to: Some("John".to_string()),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.chat_id, 1);
    }

    #[tokio::test]
    async fn send_by_username() {
        let client = MockClient::default();
        let result = send_message(
            &client,
            SendTarget::Username("johndoe".to_string()),
            "Hello",
            None,
        )
        .await;
        assert_eq!(result.unwrap().chat_id, 1);
    }

    #[test]
    fn resolve_message_prefers_explicit_flag() {
        // When --message is given, stdin is ignored entirely (even if non-terminal).
        let got = resolve_message_from(Some("hello".to_string()), false, b"piped".as_slice())
            .expect("explicit message should resolve");
        assert_eq!(got, "hello");
    }

    #[test]
    fn resolve_message_reads_stdin_and_strips_trailing_newline() {
        let got = resolve_message_from(None, false, b"hi\n".as_slice())
            .expect("piped message should resolve");
        assert_eq!(got, "hi");
    }

    #[test]
    fn resolve_message_preserves_internal_newlines() {
        let got = resolve_message_from(None, false, b"line1\nline2\n".as_slice())
            .expect("multi-line message should resolve");
        assert_eq!(got, "line1\nline2");
    }

    #[test]
    fn resolve_message_strips_crlf() {
        let got = resolve_message_from(None, false, b"hi\r\n".as_slice())
            .expect("CRLF message should resolve");
        assert_eq!(got, "hi");
    }

    #[test]
    fn resolve_message_empty_stdin_errors() {
        let err = resolve_message_from(None, false, b"".as_slice()).unwrap_err();
        assert!(err.to_string().contains("message"));
    }

    #[test]
    fn resolve_message_whitespace_only_stdin_errors() {
        let err = resolve_message_from(None, false, b"   \n".as_slice()).unwrap_err();
        assert!(err.to_string().contains("message"));
    }

    #[test]
    fn resolve_message_terminal_without_flag_errors() {
        // Interactive terminal with no --message must not hang; it errors instead.
        let err = resolve_message_from(None, true, b"".as_slice()).unwrap_err();
        assert!(err.to_string().contains("message"));
    }

    #[tokio::test]
    async fn handle_requires_recipient() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err();
        assert!(err.to_string().contains("one of"));
    }

    fn send_req(args: serde_json::Value) -> std::result::Result<SendRequest, serde_json::Error> {
        serde_json::from_value(args)
    }

    #[test]
    fn send_request_defaults_parse_mode_to_none() {
        // Back-compat: an existing caller's exact payload still deserialises and
        // still means plain text.
        let req = send_req(json!({"message": "hi", "id": 1})).unwrap();
        assert!(req.parse_mode.is_none());
    }

    #[test]
    fn send_request_accepts_explicit_null_parse_mode() {
        // This is literally what the `tg` CLI serialises when the flag is absent
        // (SendRequest has no `skip_serializing_if`), so it must not be an error.
        let req = send_req(json!({"message": "hi", "id": 1, "parse_mode": null})).unwrap();
        assert!(req.parse_mode.is_none());
    }

    #[test]
    fn send_request_rejects_unknown_field() {
        // `as` specifically: it is the one arg a live socket caller can emit, and
        // dropping it silently would send from the wrong identity with `ok:true`.
        let err = send_req(json!({"message": "hi", "id": 1, "as": "@bot"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `as`"), "{err}");
        for field in ["message", "name", "id", "to", "group", "parse_mode"] {
            assert!(err.contains(field), "error should name `{field}`: {err}");
        }
    }

    #[tokio::test]
    async fn handle_without_parse_mode_passes_none_to_client() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            ..Default::default()
        };
        handle(&client, req).await.unwrap();
        assert_eq!(
            *client.sent.lock().unwrap(),
            vec![(123, "hi".to_string(), None)]
        );
    }

    #[tokio::test]
    async fn handle_with_html_passes_html_to_client() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "<b>hi</b>".to_string(),
            id: Some(123),
            parse_mode: Some("HTML".to_string()),
            ..Default::default()
        };
        handle(&client, req).await.unwrap();
        assert_eq!(
            *client.sent.lock().unwrap(),
            vec![(
                123,
                "<b>hi</b>".to_string(),
                Some(crate::parse_mode::ParseMode::Html)
            )]
        );
    }

    #[tokio::test]
    async fn handle_with_markdown_v2_passes_markdown_to_client() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "*hi*".to_string(),
            id: Some(123),
            parse_mode: Some("MarkdownV2".to_string()),
            ..Default::default()
        };
        handle(&client, req).await.unwrap();
        assert_eq!(
            *client.sent.lock().unwrap(),
            vec![(
                123,
                "*hi*".to_string(),
                Some(crate::parse_mode::ParseMode::MarkdownV2)
            )]
        );
    }

    #[tokio::test]
    async fn handle_rejects_invalid_parse_mode() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            parse_mode: Some("markdown".to_string()),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid parse_mode 'markdown'. Expected `HTML` or `MarkdownV2`"
        );
        // The real assertion: a refused parse mode sends nothing.
        assert!(client.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_validates_parse_mode_before_resolving_target() {
        // A bad parse_mode with no recipient must report the parse_mode, not the
        // missing recipient. Deploy probes against a live daemon rely on this.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            parse_mode: Some("markdown".to_string()),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("invalid parse_mode"), "{err}");
        assert!(!err.contains("one of"), "{err}");
    }

    #[tokio::test]
    async fn handle_validates_parse_mode_before_contact_lookup() {
        // Same invariant through the resolution path: no contact search is issued
        // for a request that was already known to be malformed.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            name: Some("Unknown".to_string()),
            parse_mode: Some("html".to_string()),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err();
        assert!(matches!(err, TgError::Other(ref m) if m.contains("invalid parse_mode")));
        assert!(!matches!(err, TgError::ContactNotFound(_)));
    }

    #[tokio::test]
    async fn send_message_threads_parse_mode_through_target_resolution() {
        let client = MockClient::default();
        send_message(
            &client,
            SendTarget::Username("johndoe".to_string()),
            "Hello",
            Some(crate::parse_mode::ParseMode::Html),
        )
        .await
        .unwrap();
        let sent = client.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 1);
        assert_eq!(sent[0].2, Some(crate::parse_mode::ParseMode::Html));
    }

    #[test]
    fn send_request_from_args_carries_parse_mode() {
        use clap::Parser;
        let args = SendArgs::parse_from([
            "send",
            "--to",
            "@johndoe",
            "-m",
            "hi",
            "--parse-mode",
            "HTML",
        ]);
        let req = SendRequest::from(args);
        assert_eq!(req.parse_mode.as_deref(), Some("HTML"));
        assert_eq!(req.to.as_deref(), Some("@johndoe"));
    }
}
