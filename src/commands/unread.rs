use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ChatInfo;

pub async fn list_unread<C: TelegramClient>(client: &C, limit: i32) -> Result<Vec<ChatInfo>> {
    client.get_unread_chats(limit).await
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
}
