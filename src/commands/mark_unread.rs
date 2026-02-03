use crate::client::TelegramClient;
use crate::error::Result;

pub enum ChatTarget {
    Id(i64),
    Name(String),
}

pub async fn mark_as_unread<C: TelegramClient>(client: &C, target: ChatTarget) -> Result<()> {
    let chat_id = match target {
        ChatTarget::Id(id) => id,
        ChatTarget::Name(name) => client.find_chat_by_name(&name).await?,
    };

    client.mark_chat_as_unread(chat_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn mark_unread_by_id() {
        let client = MockClient::default();
        let result = mark_as_unread(&client, ChatTarget::Id(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mark_unread_by_name() {
        let client = MockClient::default();
        let result = mark_as_unread(&client, ChatTarget::Name("John".to_string())).await;
        assert!(result.is_ok());
    }
}
