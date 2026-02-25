use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::SendResult;

pub enum SendTarget {
    Id(i64),
    Name(String),
    Group(String),
}

pub async fn send_message<C: TelegramClient>(
    client: &C,
    target: SendTarget,
    message: &str,
) -> Result<SendResult> {
    let chat_id = match target {
        SendTarget::Id(id) => id,
        SendTarget::Name(name) => client.find_chat_by_name(&name).await?,
        SendTarget::Group(name) => client.find_group_by_name(&name).await?,
    };

    client.send_message(chat_id, message).await
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
