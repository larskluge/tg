use serde::{Deserialize, Serialize};

use crate::cli::MarkUnreadArgs;
use crate::client::TelegramClient;
use crate::error::{Result, TgError};

pub enum ChatTarget {
    Id(i64),
    Name(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarkUnreadRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub id: Option<i64>,
}

impl From<MarkUnreadArgs> for MarkUnreadRequest {
    fn from(args: MarkUnreadArgs) -> Self {
        Self {
            name: args.name,
            id: args.id,
        }
    }
}

pub async fn mark_as_unread<C: TelegramClient>(client: &C, target: ChatTarget) -> Result<()> {
    let chat_id = match target {
        ChatTarget::Id(id) => id,
        ChatTarget::Name(name) => client.find_chat_by_name(&name).await?,
    };

    client.mark_chat_as_unread(chat_id).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: MarkUnreadRequest) -> Result<()> {
    let target = if let Some(id) = req.id {
        ChatTarget::Id(id)
    } else if let Some(name) = req.name {
        ChatTarget::Name(name)
    } else {
        return Err(TgError::Other(
            "mark_unread: either `id` or `name` is required".to_string(),
        ));
    };
    mark_as_unread(client, target).await
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

    #[tokio::test]
    async fn handle_by_id() {
        let client = MockClient::default();
        let req = MarkUnreadRequest {
            id: Some(1),
            ..Default::default()
        };
        handle(&client, req).await.unwrap();
    }

    #[tokio::test]
    async fn handle_requires_id_or_name() {
        let client = MockClient::default();
        let err = handle(&client, MarkUnreadRequest::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("either `id` or `name`"));
    }
}
