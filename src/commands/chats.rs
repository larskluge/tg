use serde::{Deserialize, Serialize};

use crate::cli::ChatsArgs;
use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ChatInfo;

fn default_limit() -> i32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatsRequest {
    #[serde(default = "default_limit")]
    pub limit: i32,
}

impl Default for ChatsRequest {
    fn default() -> Self {
        Self {
            limit: default_limit(),
        }
    }
}

impl From<ChatsArgs> for ChatsRequest {
    fn from(args: ChatsArgs) -> Self {
        Self { limit: args.limit }
    }
}

pub async fn list_chats<C: TelegramClient>(client: &C, limit: i32) -> Result<Vec<ChatInfo>> {
    client.get_chats(limit).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: ChatsRequest) -> Result<Vec<ChatInfo>> {
    list_chats(client, req.limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn list_chats_returns_chats() {
        let client = MockClient::default();
        let chats = list_chats(&client, 50).await.unwrap();
        assert_eq!(chats.len(), 2);
        assert_eq!(chats[0].name, "John Doe");
    }

    #[tokio::test]
    async fn list_chats_respects_limit() {
        let client = MockClient::default();
        let chats = list_chats(&client, 1).await.unwrap();
        assert_eq!(chats.len(), 1);
    }

    #[tokio::test]
    async fn handle_uses_request_limit() {
        let client = MockClient::default();
        let chats = handle(&client, ChatsRequest { limit: 1 }).await.unwrap();
        assert_eq!(chats.len(), 1);
    }

    #[test]
    fn request_default_limit_is_50() {
        assert_eq!(ChatsRequest::default().limit, 50);
    }

    #[test]
    fn request_deserializes_with_missing_limit() {
        let req: ChatsRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(req.limit, 50);
    }
}
