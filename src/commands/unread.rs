use serde::{Deserialize, Serialize};

use crate::cli::UnreadArgs;
use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ChatInfo;

fn default_limit() -> i32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnreadRequest {
    #[serde(default = "default_limit")]
    pub limit: i32,
}

impl Default for UnreadRequest {
    fn default() -> Self {
        Self {
            limit: default_limit(),
        }
    }
}

impl From<UnreadArgs> for UnreadRequest {
    fn from(args: UnreadArgs) -> Self {
        Self { limit: args.limit }
    }
}

pub async fn list_unread<C: TelegramClient>(client: &C, limit: i32) -> Result<Vec<ChatInfo>> {
    client.get_unread_chats(limit).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: UnreadRequest) -> Result<Vec<ChatInfo>> {
    list_unread(client, req.limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn list_unread_returns_unread_chats() {
        let client = MockClient::default();
        let chats = list_unread(&client, 50).await.unwrap();
        // Should include John Doe (unread_count: 2) and Family Chat (unread_count: 5)
        assert_eq!(chats.len(), 2);
        assert!(chats.iter().all(|c| c.unread_count > 0));
    }

    #[tokio::test]
    async fn handle_uses_request_limit() {
        let client = MockClient::default();
        let chats = handle(&client, UnreadRequest { limit: 50 }).await.unwrap();
        assert_eq!(chats.len(), 2);
    }
}
