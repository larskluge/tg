use serde::{Deserialize, Serialize};

use crate::cli::GroupsArgs;
use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ChatInfo;

fn default_limit() -> i32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupsRequest {
    #[serde(default = "default_limit")]
    pub limit: i32,
}

impl Default for GroupsRequest {
    fn default() -> Self {
        Self {
            limit: default_limit(),
        }
    }
}

impl From<GroupsArgs> for GroupsRequest {
    fn from(args: GroupsArgs) -> Self {
        Self { limit: args.limit }
    }
}

pub async fn list_groups<C: TelegramClient>(client: &C, limit: i32) -> Result<Vec<ChatInfo>> {
    client.get_groups(limit).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: GroupsRequest) -> Result<Vec<ChatInfo>> {
    list_groups(client, req.limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn list_groups_returns_groups() {
        let client = MockClient::default();
        let groups = list_groups(&client, 50).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Family Chat");
    }

    #[tokio::test]
    async fn handle_uses_request_limit() {
        let client = MockClient::default();
        let groups = handle(&client, GroupsRequest { limit: 50 }).await.unwrap();
        assert_eq!(groups.len(), 1);
    }
}
