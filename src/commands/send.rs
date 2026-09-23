use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::cli::SendArgs;
use crate::client::TelegramClient;
use crate::error::{Result, TgError};
use crate::media::{MediaFile, SendFile, validate_files};
use crate::output::SendResult;
use crate::parse_mode::ParseMode;
use crate::resolve::Recipient;

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
    /// 1-10 local files to send as one message, with `message` as the caption.
    /// Absent is a text-only send and is byte-identical to the pre-`files`
    /// contract; an empty array is refused. Validated in [`handle`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<SendFile>>,
    /// The TDLib id of a message in the DESTINATION chat to send this one as a
    /// native Telegram reply to — the id `tg messages`/`tg sync` print, i.e.
    /// `server_id << 20`, never the bare server id a `t.me` link shows. Absent is
    /// an ordinary send and is byte-identical to the pre-reply contract.
    ///
    /// The target is proved to exist in that chat and to accept a reply
    /// ([`TelegramClient::check_reply_target`]) before anything is sent, because
    /// TDLib's own answer to a reply it cannot honour is to send the message
    /// anyway, unthreaded — a delivery the caller cannot take back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<i64>,
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
            // The CLI has no attachment flag; attachments are a socket-protocol
            // feature. `skip_serializing_if` keeps the key off the wire
            // entirely, so a new CLI still talks to an older daemon.
            files: None,
            // Likewise for replies: a socket-protocol feature, absent from the
            // wire unless set.
            reply_to: None,
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

/// Turn a [`SendTarget`] into a chat id. This is the step that costs TDLib
/// contact and public-chat searches, which is why every request check runs
/// before it.
async fn resolve_chat_id<C: TelegramClient>(client: &C, target: SendTarget) -> Result<i64> {
    match target {
        SendTarget::Id(id) => Ok(id),
        SendTarget::Name(name) => client.find_chat_by_name(&name).await,
        SendTarget::Username(username) => client.find_chat_by_username(&username).await,
        SendTarget::Group(name) => client.find_group_by_name(&name).await,
    }
}

pub async fn send_message<C: TelegramClient>(
    client: &C,
    target: SendTarget,
    message: &str,
    parse_mode: Option<ParseMode>,
    reply_to: Option<i64>,
) -> Result<SendResult> {
    let chat_id = resolve_chat_id(client, target).await?;
    check_reply(client, chat_id, reply_to).await?;

    client
        .send_message(chat_id, message, parse_mode, reply_to)
        .await
}

/// Prove a reply target before the send that would reply to it. It runs after
/// the chat is resolved because the check is "this message is in THAT chat": a
/// TDLib message id is only meaningful within one chat, and the same number can
/// name an unrelated message in another.
async fn check_reply<C: TelegramClient>(
    client: &C,
    chat_id: i64,
    reply_to: Option<i64>,
) -> Result<()> {
    match reply_to {
        Some(message_id) => client.check_reply_target(chat_id, message_id).await,
        None => Ok(()),
    }
}

/// Send 1-10 validated files as one message, `caption` being the message's
/// caption. Kept separate from [`send_message`] so the text path's call into
/// the client is unchanged.
pub async fn send_media<C: TelegramClient>(
    client: &C,
    target: SendTarget,
    caption: &str,
    parse_mode: Option<ParseMode>,
    files: &[MediaFile],
    reply_to: Option<i64>,
) -> Result<SendResult> {
    let chat_id = resolve_chat_id(client, target).await?;
    check_reply(client, chat_id, reply_to).await?;

    client
        .send_media_message(chat_id, caption, parse_mode, files, reply_to)
        .await
}

/// Plan a bot (`--as`) send: validate the request and say which recipient form
/// was asked for, doing no I/O at all.
///
/// Bot sends go out over the HTTP Bot API and never reach [`handle`], so this is
/// the bot path's copy of the same invariant: a malformed request must never
/// cost a round trip. It matters more here than on the socket, because
/// `resolve::resolve_recipient` cold-starts TDLib, performs a Telegram lookup
/// for an unknown `@username` and writes the resolved contact back to
/// `credentials.json` — side effects a rejected request must not pay for.
///
/// Returning the recipient *from the call that validates* is what enforces the
/// ordering: `run_bot_send` cannot resolve a recipient it has not been handed.
pub fn plan_bot_send(args: &SendArgs) -> Result<(Recipient, Option<ParseMode>)> {
    let parse_mode = args
        .parse_mode
        .as_deref()
        .map(ParseMode::parse)
        .transpose()?;

    let recipient = if let Some(ref to) = args.to {
        Recipient::To(to.clone())
    } else if let Some(id) = args.id {
        Recipient::Id(id)
    } else if let Some(ref group) = args.group {
        Recipient::Group(group.clone())
    } else if let Some(ref name) = args.name {
        Recipient::Name(name.clone())
    } else {
        return Err(TgError::Other(
            "send: one of `id`, `to`, `group`, or `name` is required".to_string(),
        ));
    };

    Ok((recipient, parse_mode))
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

    // Attachments are checked here for the same reason, plus one of their own:
    // TDLib uploads a file *after* the send call returns, so a path it cannot
    // read would otherwise surface as a failed send on a message the caller was
    // already told about. Pinned by
    // `handle_validates_files_before_resolving_target`.
    let files = validate_files(req.files.as_deref())?;

    // And the reply target's shape, for the same reason. Only its SHAPE: whether
    // it names a message in the chat needs the chat, so that half runs after the
    // ladder. Pinned by `handle_validates_reply_to_before_resolving_target`.
    let reply_to = validate_reply_to(req.reply_to)?;

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

    if files.is_empty() {
        send_message(client, target, &req.message, parse_mode, reply_to).await
    } else {
        send_media(client, target, &req.message, parse_mode, &files, reply_to).await
    }
}

/// TDLib gives a server message the id `server_id << 20`, so every id a reply
/// can name is a positive multiple of 2^20. Anything else is a caller holding
/// the wrong number — most likely the bare server id from a `t.me` link, which
/// TDLib would read as a different (usually nonexistent) message. Refused by
/// name rather than left to a lookup whose "not found" would not say why.
fn validate_reply_to(reply_to: Option<i64>) -> Result<Option<i64>> {
    const SERVER_ID_SHIFT: i64 = 1 << 20;
    match reply_to {
        None => Ok(None),
        Some(id) if id > 0 && id % SERVER_ID_SHIFT == 0 => Ok(Some(id)),
        Some(id) => Err(TgError::Other(format!(
            "invalid reply_to {id}: expected a TDLib message id (server_id << 20, as `tg messages` prints it), \
             not a bare server id or a local message id"
        ))),
    }
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
        let result = send_message(&client, SendTarget::Id(123), "Hello", None, None).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().chat_id, 123);
    }

    #[tokio::test]
    async fn send_by_name() {
        let client = MockClient::default();
        let result = send_message(
            &client,
            SendTarget::Name("John".to_string()),
            "Hello",
            None,
            None,
        )
        .await;
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
        // `null` means absent, deliberately, even though `""` is refused. It is
        // what an unset optional serialises to in Go (`map[string]any` with a
        // missing key, or a `*string` without `omitempty`), in Python, and in
        // `tg`'s own CLI proxy, which has no `skip_serializing_if`. Refusing it
        // would turn every plain-text send from those callers into a hard error
        // while closing nothing: a caller that omits the key entirely still gets
        // a plain send, and "absent means plain" is the contract.
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
        for field in [
            "message",
            "name",
            "id",
            "to",
            "group",
            "parse_mode",
            "files",
            "reply_to",
        ] {
            assert!(err.contains(field), "error should name `{field}`: {err}");
        }
    }

    // Fictional TDLib ids: a DM with user 5550001 and a supergroup, each holding
    // one message. TDLib numbers server messages `server_id << 20`, and the SAME
    // number can exist in two chats — which is what the chat check is for.
    const DM: i64 = 5_550_001;
    const GROUP: i64 = -1_009_876_543_210;
    const DM_MSG: i64 = 4_211 << 20;
    const GROUP_MSG: i64 = 918 << 20;

    fn client_with_reply_targets() -> MockClient {
        let mut client = MockClient::default();
        for (chat_id, id) in [(DM, DM_MSG), (GROUP, GROUP_MSG)] {
            let mut m = client.messages[0].clone();
            m.chat_id = chat_id;
            m.id = id;
            client.messages.push(m);
        }
        client
    }

    #[test]
    fn send_request_reply_to_is_optional_and_off_the_wire_when_absent() {
        let req = send_req(json!({"message": "hi", "id": 1})).unwrap();
        assert!(req.reply_to.is_none());
        let wire = serde_json::to_value(&req).unwrap();
        assert!(wire.get("reply_to").is_none(), "{wire}");

        let req = send_req(json!({"message": "hi", "id": DM, "reply_to": DM_MSG})).unwrap();
        assert_eq!(req.reply_to, Some(DM_MSG));
    }

    #[tokio::test]
    async fn handle_validates_reply_to_before_resolving_target() {
        // A bare server id (what a t.me link shows) is the likely mistake. With no
        // recipient, the error must be about reply_to — so a malformed request
        // costs no TDLib lookup, and a probe can tell this build from an older one.
        let client = MockClient::default();
        for bad in [4_211, 0, -(4_211 << 20)] {
            let req = SendRequest {
                message: "hi".to_string(),
                reply_to: Some(bad),
                ..Default::default()
            };
            let err = handle(&client, req).await.unwrap_err().to_string();
            assert!(err.contains("invalid reply_to"), "{bad}: {err}");
            assert!(!err.contains("one of"), "{bad}: {err}");
        }
        assert!(client.reply_checks.lock().unwrap().is_empty());
        assert!(client.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_sends_a_native_reply_in_the_chat_that_holds_the_target() {
        let client = client_with_reply_targets();
        let req = SendRequest {
            message: "thanks".to_string(),
            id: Some(GROUP),
            reply_to: Some(GROUP_MSG),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.reply_to_message_id, Some(GROUP_MSG));
        assert_eq!(
            *client.reply_checks.lock().unwrap(),
            vec![(GROUP, GROUP_MSG)]
        );
        assert_eq!(*client.replies_sent.lock().unwrap(), vec![Some(GROUP_MSG)]);
        let wire = serde_json::to_value(&res).unwrap();
        assert_eq!(wire["reply_to_message_id"], json!(GROUP_MSG));
    }

    #[tokio::test]
    async fn handle_refuses_a_target_from_another_chat_and_sends_nothing() {
        // DM_MSG is a real message — in the DM. Replying to it in the group is the
        // mismatch TDLib would answer with an unthreaded message, so it must be
        // refused before the send.
        let client = client_with_reply_targets();
        let req = SendRequest {
            message: "thanks".to_string(),
            id: Some(GROUP),
            reply_to: Some(DM_MSG),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("not accessible in chat"), "{err}");
        assert!(client.sent.lock().unwrap().is_empty());
        assert!(client.replies_sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_media_send_carries_the_reply_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"%PDF-1.4 fictional").unwrap();
        let client = client_with_reply_targets();
        let req = SendRequest {
            message: "the file you asked for".to_string(),
            id: Some(DM),
            reply_to: Some(DM_MSG),
            files: Some(vec![SendFile {
                path: path.to_string_lossy().into_owned(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.reply_to_message_id, Some(DM_MSG));
        assert_eq!(client.media_sent.lock().unwrap().len(), 1);
        assert_eq!(*client.replies_sent.lock().unwrap(), vec![Some(DM_MSG)]);
    }

    #[tokio::test]
    async fn an_ordinary_send_checks_nothing_and_replies_to_nothing() {
        let client = client_with_reply_targets();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(DM),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert!(client.reply_checks.lock().unwrap().is_empty());
        assert_eq!(*client.replies_sent.lock().unwrap(), vec![None]);
        let wire = serde_json::to_value(&res).unwrap();
        assert!(wire.get("reply_to_message_id").is_none(), "{wire}");
    }

    #[test]
    fn the_cli_never_puts_reply_to_on_the_wire() {
        use clap::Parser;
        let req = SendRequest::from(SendArgs::parse_from([
            "send", "--id", "5550001", "-m", "hi",
        ]));
        assert!(req.reply_to.is_none());
        let wire = serde_json::to_value(&req).unwrap();
        assert!(wire.get("reply_to").is_none(), "{wire}");
    }

    fn bot_args(argv: &[&str]) -> SendArgs {
        use clap::Parser;
        match crate::cli::Cli::parse_from(argv).command {
            crate::cli::Command::Send(args) => args,
            _ => panic!("expected a send command"),
        }
    }

    #[test]
    fn plan_bot_send_validates_parse_mode_before_naming_a_recipient() {
        // The bot path's copy of `handle`'s invariant. `resolve_recipient` cold-starts
        // TDLib, hits the network for an unknown @username and persists the resolved
        // contact to credentials.json — a malformed request must cost none of that.
        // The ordering is enforced by the data dependency: `run_bot_send` cannot
        // resolve a recipient it has not been handed, and it is handed one only
        // after the parse mode has been checked.
        let err = plan_bot_send(&bot_args(&[
            "tg",
            "send",
            "--as",
            "@mybot",
            "--to",
            "@someone",
            "--parse-mode",
            "html",
            "-m",
            "<b>x</b>",
        ]))
        .unwrap_err()
        .to_string();
        assert_eq!(
            err,
            "invalid parse_mode 'html'. Expected `HTML` or `MarkdownV2`"
        );
    }

    #[test]
    fn plan_bot_send_reports_the_parse_mode_before_a_missing_recipient() {
        // Both are caller bugs; reporting the parse mode first is what proves the
        // check runs before the recipient is even looked at.
        let err = plan_bot_send(&bot_args(&[
            "tg",
            "send",
            "--as",
            "@mybot",
            "--id",
            "42",
            "--parse-mode",
            "md",
            "-m",
            "x",
        ]))
        .unwrap_err()
        .to_string();
        assert!(err.contains("invalid parse_mode 'md'"), "{err}");
    }

    #[test]
    fn plan_bot_send_returns_recipient_and_mode() {
        let (recipient, mode) = plan_bot_send(&bot_args(&[
            "tg",
            "send",
            "--as",
            "@mybot",
            "--to",
            "@someone",
            "--parse-mode",
            "HTML",
            "-m",
            "<b>x</b>",
        ]))
        .expect("a well-formed bot send must plan");
        assert_eq!(mode, Some(ParseMode::Html));
        match recipient {
            Recipient::To(to) => assert_eq!(to, "@someone"),
            _ => panic!("--to must produce Recipient::To"),
        }
    }

    #[test]
    fn plan_bot_send_requires_a_recipient() {
        let mut args = bot_args(&["tg", "send", "--as", "@mybot", "--id", "42", "-m", "x"]);
        args.id = None;
        let err = plan_bot_send(&args).unwrap_err().to_string();
        assert!(err.contains("one of"), "{err}");
    }

    #[test]
    fn plan_bot_send_recipient_ladder_matches_the_socket_path() {
        // `--to` wins over `--id`, `--id` over `--group`, `--group` over a bare
        // name — the same order `handle` uses, so the two paths cannot drift.
        let mut args = bot_args(&[
            "tg", "send", "Jane", "--as", "@b", "--to", "@x", "--id", "7", "--group", "G", "-m",
            "m",
        ]);
        assert!(matches!(plan_bot_send(&args).unwrap().0, Recipient::To(_)));
        args.to = None;
        assert!(matches!(plan_bot_send(&args).unwrap().0, Recipient::Id(7)));
        args.id = None;
        assert!(matches!(
            plan_bot_send(&args).unwrap().0,
            Recipient::Group(_)
        ));
        args.group = None;
        assert!(matches!(
            plan_bot_send(&args).unwrap().0,
            Recipient::Name(_)
        ));
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
            None,
        )
        .await
        .unwrap();
        let sent = client.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 1);
        assert_eq!(sent[0].2, Some(crate::parse_mode::ParseMode::Html));
    }

    fn media_dir() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn media_file(dir: &tempfile::TempDir, name: &str) -> String {
        let path = dir.path().join(name);
        std::fs::write(&path, b"bytes").unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn send_request_without_files_deserialises_and_means_text() {
        // The whole back-compat guarantee: an existing caller's exact payload
        // still parses, and nothing about it says "media".
        let req = send_req(json!({"message": "hi", "id": 1})).unwrap();
        assert!(req.files.is_none());
    }

    #[test]
    fn send_request_accepts_explicit_null_files() {
        // Same reasoning as `parse_mode: null`: an unset optional serialises to
        // `null` in Go and Python, and refusing it would turn every text send
        // from those callers into a hard error while closing nothing.
        let req = send_req(json!({"message": "hi", "id": 1, "files": null})).unwrap();
        assert!(req.files.is_none());
    }

    #[test]
    fn send_request_from_args_never_carries_files() {
        // The CLI has no attachment flag, and the key must stay off the wire so
        // a new CLI still talks to a daemon that predates `files`.
        use clap::Parser;
        let req = SendRequest::from(SendArgs::parse_from(["send", "--id", "1", "-m", "hi"]));
        assert!(req.files.is_none());
        let wire = serde_json::to_value(&req).unwrap();
        assert!(
            wire.get("files").is_none(),
            "`files` must be omitted, not null: {wire}"
        );
    }

    #[tokio::test]
    async fn text_only_request_still_reaches_the_text_send_path() {
        // Byte-identical behaviour: the same recording, and nothing on the
        // media path.
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
        assert!(client.media_sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_sends_a_single_file_with_the_body_as_caption() {
        let dir = media_dir();
        let path = media_file(&dir, "Q3 report.pdf");
        let client = MockClient::default();
        let req = SendRequest {
            message: "here it is".to_string(),
            id: Some(123),
            files: Some(vec![SendFile {
                path: path.clone(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.chat_id, 123);

        let media = client.media_sent.lock().unwrap();
        assert_eq!(media.len(), 1);
        let (chat_id, caption, parse_mode, files) = &media[0];
        assert_eq!(*chat_id, 123);
        assert_eq!(caption, "here it is");
        assert_eq!(*parse_mode, None);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.to_string_lossy(), path);
        assert_eq!(files[0].kind, crate::media::MediaKind::File);
        // The basename is the only filename Telegram can show, so it must be
        // carried through untouched — spaces included.
        assert_eq!(files[0].display_name(), "Q3 report.pdf");
        // A media send must not also be recorded as a text send.
        assert!(client.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_sends_multiple_files_as_one_album_with_one_caption() {
        // The caption is a property of the message, not of each file: the
        // client is handed it once, alongside the ordered file set.
        let dir = media_dir();
        let first = media_file(&dir, "a.jpg");
        let second = media_file(&dir, "b.jpg");
        let client = MockClient::default();
        let req = SendRequest {
            message: "two photos".to_string(),
            id: Some(123),
            parse_mode: Some("HTML".to_string()),
            files: Some(vec![
                SendFile {
                    path: first.clone(),
                    kind: Some("photo".to_string()),
                    width: Some(800),
                    height: Some(600),
                },
                SendFile {
                    path: second.clone(),
                    kind: Some("photo".to_string()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        handle(&client, req).await.unwrap();

        let media = client.media_sent.lock().unwrap();
        assert_eq!(media.len(), 1, "one album is one send, not one per file");
        let (_, caption, parse_mode, files) = &media[0];
        assert_eq!(caption, "two photos");
        assert_eq!(*parse_mode, Some(ParseMode::Html));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path.to_string_lossy(), first);
        assert_eq!(files[1].path.to_string_lossy(), second);
        assert_eq!((files[0].width, files[0].height), (800, 600));
        // Absent dimensions mean "let Telegram work it out".
        assert_eq!((files[1].width, files[1].height), (0, 0));
    }

    #[tokio::test]
    async fn handle_refuses_mixed_kinds_and_sends_nothing() {
        let dir = media_dir();
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            files: Some(vec![
                SendFile {
                    path: media_file(&dir, "a.jpg"),
                    kind: Some("photo".to_string()),
                    ..Default::default()
                },
                SendFile {
                    path: media_file(&dir, "b.pdf"),
                    kind: Some("file".to_string()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("mixes kind"), "{err}");
        assert!(client.media_sent.lock().unwrap().is_empty());
        assert!(client.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_refuses_more_than_ten_files_and_sends_nothing() {
        let dir = media_dir();
        let path = media_file(&dir, "a.pdf");
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            files: Some(
                (0..11)
                    .map(|_| SendFile {
                        path: path.clone(),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("at most 10"), "{err}");
        assert!(client.media_sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_refuses_a_missing_file_and_sends_nothing() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            files: Some(vec![SendFile {
                path: "/nonexistent/report.pdf".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("unreadable"), "{err}");
        assert!(client.media_sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_refuses_a_relative_path_and_sends_nothing() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            files: Some(vec![SendFile {
                path: "report.pdf".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("is not absolute"), "{err}");
        assert!(client.media_sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_refuses_an_empty_files_array() {
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            files: Some(vec![]),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("present but empty"), "{err}");
        // Not re-read as a text send: the caller described a message it did not
        // get, and a silent reinterpretation is what hides that.
        assert!(client.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_validates_files_before_resolving_target() {
        // The `files` half of `handle_validates_parse_mode_before_resolving_target`:
        // a malformed attachment set with no recipient must report the files,
        // not the missing recipient — no TDLib lookup is spent on a request
        // already known to be bad.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            files: Some(vec![SendFile {
                path: "relative.pdf".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("is not absolute"), "{err}");
        assert!(!err.contains("one of"), "{err}");
    }

    #[tokio::test]
    async fn handle_validates_files_before_contact_lookup() {
        // Same invariant through the resolution path: no contact search is
        // issued for a request whose attachments are already known to be bad.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            name: Some("Unknown".to_string()),
            files: Some(vec![SendFile {
                path: "/nonexistent/a.pdf".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err();
        assert!(matches!(err, TgError::Other(ref m) if m.contains("unreadable")));
        assert!(!matches!(err, TgError::ContactNotFound(_)));
    }

    #[tokio::test]
    async fn handle_validates_parse_mode_before_files() {
        // Both are caller bugs and both are cheap, but the parse mode is
        // reported first so the existing probe contract is unchanged by the
        // arrival of `files`.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            parse_mode: Some("markdown".to_string()),
            files: Some(vec![SendFile {
                path: "relative.pdf".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.contains("invalid parse_mode"), "{err}");
    }

    #[tokio::test]
    async fn media_send_resolves_the_recipient_the_same_way_text_does() {
        // One ladder for both paths: an `@username` must resolve by username on
        // a media send too, not by display name.
        let dir = media_dir();
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            to: Some("@johndoe".to_string()),
            files: Some(vec![SendFile {
                path: media_file(&dir, "a.pdf"),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert_eq!(res.chat_id, 1);
        assert_eq!(client.media_sent.lock().unwrap()[0].0, 1);
    }

    #[tokio::test]
    async fn a_retry_of_the_one_remaining_file_is_a_single_file_send() {
        // The caller's half of a partial-delivery retry: it resends only the
        // elements that never arrived, which is commonly ONE file. Nothing here
        // special-cases that — the request is an ordinary one-entry `files`
        // array — and it must not become a one-element album, which TDLib
        // refuses (see `client::tests::a_single_file_is_not_an_album` for the
        // branch that keeps it off `sendMessageAlbum`).
        let dir = media_dir();
        let remaining = media_file(&dir, "chart.png");
        let client = MockClient::default();
        let req = SendRequest {
            message: "the one that didn't arrive".to_string(),
            id: Some(123),
            files: Some(vec![SendFile {
                path: remaining.clone(),
                kind: Some("photo".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();

        let media = client.media_sent.lock().unwrap();
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].3.len(), 1, "one file is one element");
        assert_eq!(media[0].3[0].path.to_string_lossy(), remaining);
        assert!(client.sent.lock().unwrap().is_empty());

        // The record is indexed within THIS request, not within the original
        // send: the caller re-reads its own `files` order either way.
        let elements = res.elements.expect("a media send reports its elements");
        assert_eq!(elements.delivered.len(), 1);
        assert_eq!(elements.delivered[0].index, 0);
        assert_eq!(res.message_id, elements.delivered[0].message_id);
    }

    #[tokio::test]
    async fn a_text_send_reports_no_per_element_data() {
        // The text path is untouched by the media record: there is no element
        // to report on, and a caller reading `elements` as "this was a media
        // send" must not find one here.
        let client = MockClient::default();
        let req = SendRequest {
            message: "hi".to_string(),
            id: Some(123),
            ..Default::default()
        };
        let res = handle(&client, req).await.unwrap();
        assert!(res.elements.is_none());
        assert_eq!(
            serde_json::to_value(&res).unwrap(),
            json!({"message_id": 12345, "chat_id": 123})
        );
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
