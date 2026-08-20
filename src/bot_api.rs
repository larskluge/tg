use crate::error::{Result, TgError};
use crate::parse_mode::ParseMode;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    description: Option<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
pub struct BotUser {
    pub id: i64,
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SentMessage {
    pub message_id: i64,
}

fn api_url(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{token}/{method}")
}

fn parse_response<T: serde::de::DeserializeOwned>(resp: TelegramResponse<T>) -> Result<T> {
    if resp.ok {
        resp.result
            .ok_or_else(|| TgError::Other("Bot API returned ok but no result".to_string()))
    } else {
        Err(TgError::Other(format!(
            "Bot API error: {}",
            resp.description.unwrap_or_else(|| "unknown".to_string())
        )))
    }
}

fn http_client() -> reqwest::Client {
    // Reuse via once_cell to avoid creating a new client (and connection pool) per call.
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new).clone()
}

pub async fn get_me(token: &str) -> Result<BotUser> {
    let resp: TelegramResponse<BotUser> = http_client()
        .get(api_url(token, "getMe"))
        .send()
        .await
        .map_err(|e| TgError::Other(format!("HTTP request failed: {e}")))?
        .json()
        .await
        .map_err(|e| TgError::Other(format!("Failed to parse Bot API response: {e}")))?;

    parse_response(resp)
}

/// Build the `sendMessage` body. Extracted so the payload shape is testable
/// without HTTP. When `parse_mode` is `None` the key is **omitted**, not sent as
/// `null`, so an unformatted bot send is byte-for-byte what it was before this
/// flag existed — the live notifier path depends on that.
fn send_message_payload(
    chat_id: i64,
    text: &str,
    parse_mode: Option<ParseMode>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
    });
    if let Some(mode) = parse_mode {
        payload["parse_mode"] = serde_json::Value::String(mode.as_str().to_string());
    }
    payload
}

pub async fn send_message(
    token: &str,
    chat_id: i64,
    text: &str,
    parse_mode: Option<ParseMode>,
) -> Result<i64> {
    let resp: TelegramResponse<SentMessage> = http_client()
        .post(api_url(token, "sendMessage"))
        .json(&send_message_payload(chat_id, text, parse_mode))
        .send()
        .await
        .map_err(|e| TgError::Other(format!("HTTP request failed: {e}")))?
        .json()
        .await
        .map_err(|e| TgError::Other(format!("Failed to parse Bot API response: {e}")))?;

    let msg = parse_response(resp)?;
    Ok(msg.message_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_format() {
        assert_eq!(
            api_url("123:ABC", "getMe"),
            "https://api.telegram.org/bot123:ABC/getMe"
        );
    }

    #[test]
    fn parse_response_ok() {
        let resp = TelegramResponse {
            ok: true,
            description: None,
            result: Some(42),
        };
        assert_eq!(parse_response(resp).unwrap(), 42);
    }

    #[test]
    fn send_message_payload_omits_parse_mode_when_none() {
        // Byte-identity guarantee for existing bot callers: the key is absent,
        // not null. A `json!` literal carrying `"parse_mode": null` would pass a
        // laxer assertion while changing the payload every notifier sends.
        assert_eq!(
            send_message_payload(42, "x", None),
            serde_json::json!({"chat_id": 42, "text": "x"})
        );
    }

    #[test]
    fn send_message_payload_includes_html() {
        let payload = send_message_payload(42, "<b>x</b>", Some(ParseMode::Html));
        assert_eq!(payload["parse_mode"], "HTML");
        assert_eq!(payload["text"], "<b>x</b>");
    }

    #[test]
    fn send_message_payload_includes_markdown_v2() {
        let payload = send_message_payload(42, "*x*", Some(ParseMode::MarkdownV2));
        assert_eq!(payload["parse_mode"], "MarkdownV2");
    }

    #[test]
    fn parse_response_error() {
        let resp: TelegramResponse<i32> = TelegramResponse {
            ok: false,
            description: Some("Unauthorized".to_string()),
            result: None,
        };
        let err = parse_response(resp).unwrap_err();
        assert!(err.to_string().contains("Unauthorized"));
    }
}
