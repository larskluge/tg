use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::UserInfo;

pub async fn whoami<C: TelegramClient>(client: &C) -> Result<UserInfo> {
    client.get_me().await
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
}
