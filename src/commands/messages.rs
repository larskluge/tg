use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::MessageInfo;

pub enum ChatTarget {
    Id(i64),
    Name(String),
}

pub async fn get_messages<C: TelegramClient>(
    client: &C,
    target: ChatTarget,
    limit: i32,
) -> Result<Vec<MessageInfo>> {
    let chat_id = match target {
        ChatTarget::Id(id) => id,
        ChatTarget::Name(name) => client.find_chat_by_name(&name).await?,
    };

    client.get_messages(chat_id, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn get_messages_by_id() {
        let client = MockClient::default();
        let messages = get_messages(&client, ChatTarget::Id(1), 20).await.unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn get_messages_by_name() {
        let client = MockClient::default();
        let messages = get_messages(&client, ChatTarget::Name("John".to_string()), 20)
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn get_messages_respects_limit() {
        let client = MockClient::default();
        let messages = get_messages(&client, ChatTarget::Id(1), 1).await.unwrap();
        assert_eq!(messages.len(), 1);
    }
}
