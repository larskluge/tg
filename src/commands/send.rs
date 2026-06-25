use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::cli::SendArgs;
use crate::client::TelegramClient;
use crate::error::{Result, TgError};
use crate::output::SendResult;

pub enum SendTarget {
    Id(i64),
    Name(String),
    Username(String),
    Group(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
) -> Result<SendResult> {
    let chat_id = match target {
        SendTarget::Id(id) => id,
        SendTarget::Name(name) => client.find_chat_by_name(&name).await?,
        SendTarget::Username(username) => client.find_chat_by_username(&username).await?,
        SendTarget::Group(name) => client.find_group_by_name(&name).await?,
    };

    client.send_message(chat_id, message).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: SendRequest) -> Result<SendResult> {
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

    send_message(client, target, &req.message).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::TelegramClient;
    use crate::client::mock::MockClient;
    use crate::error::TgError;

    #[tokio::test]
    async fn send_by_id() {
        let client = MockClient::default();
        let result = send_message(&client, SendTarget::Id(123), "Hello").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().chat_id, 123);
    }

    #[tokio::test]
    async fn send_by_name() {
        let client = MockClient::default();
        let result = send_message(&client, SendTarget::Name("John".to_string()), "Hello").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_by_group() {
        let client = MockClient::default();
        let result = send_message(&client, SendTarget::Group("Family".to_string()), "Hello").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_to_unknown_contact() {
        let client = MockClient::default();
        let result = send_message(&client, SendTarget::Name("Unknown".to_string()), "Hello").await;
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
}
