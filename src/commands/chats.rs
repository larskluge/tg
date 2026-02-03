use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ChatInfo;

pub async fn list_chats<C: TelegramClient>(client: &C, limit: i32) -> Result<Vec<ChatInfo>> {
    client.get_chats(limit).await
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
}
