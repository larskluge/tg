use serde::{Deserialize, Serialize};

use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::UserInfo;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WhoamiRequest {}

pub async fn whoami<C: TelegramClient>(client: &C) -> Result<UserInfo> {
    client.get_me().await
}

pub async fn handle<C: TelegramClient>(client: &C, _req: WhoamiRequest) -> Result<UserInfo> {
    whoami(client).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn whoami_returns_user_info() {
        let client = MockClient::default();
        let info = whoami(&client).await.unwrap();
        assert_eq!(info.id, 42);
        assert_eq!(info.first_name, "John");
        assert_eq!(info.last_name, "Doe");
        assert_eq!(info.username, Some("johndoe".to_string()));
        assert_eq!(info.phone, Some("+1234567890".to_string()));
    }

    #[tokio::test]
    async fn handle_invokes_whoami() {
        let client = MockClient::default();
        let info = handle(&client, WhoamiRequest::default()).await.unwrap();
        assert_eq!(info.id, 42);
    }
}
