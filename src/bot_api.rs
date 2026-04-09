use crate::error::{Result, TgError};
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

pub async fn get_me(token: &str) -> Result<BotUser> {
    let client = reqwest::Client::new();
    let resp: TelegramResponse<BotUser> = client
        .get(api_url(token, "getMe"))
        .send()
        .await
        .map_err(|e| TgError::Other(format!("HTTP request failed: {e}")))?
        .json()
        .await
        .map_err(|e| TgError::Other(format!("Failed to parse Bot API response: {e}")))?;

    parse_response(resp)
}

pub async fn send_message(token: &str, chat_id: i64, text: &str) -> Result<i64> {
    let client = reqwest::Client::new();
    let resp: TelegramResponse<SentMessage> = client
        .post(api_url(token, "sendMessage"))
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
        }))
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
