use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ChatInfo;

pub async fn list_groups<C: TelegramClient>(client: &C, limit: i32) -> Result<Vec<ChatInfo>> {
    client.get_groups(limit).await
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
}
