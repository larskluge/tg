use crate::client::TelegramClient;
use crate::error::Result;

pub async fn authenticate<C: TelegramClient>(
    client: &mut C,
    phone: Option<&str>,
) -> Result<()> {
    client.authenticate(phone).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn auth_with_phone_sends_code() {
        let mut client = MockClient::default();
        let result = authenticate(&mut client, Some("+1234567890")).await;
        assert!(result.is_ok());
        assert!(client.phone_submitted().await);
    }

    #[tokio::test]
    async fn auth_without_phone_completes_if_code_pending() {
        let mut client = MockClient::with_state(crate::client::mock::AuthState::WaitCode);
        let result = authenticate(&mut client, None).await;
        assert!(result.is_ok());
        assert!(client.is_authenticated().await);
    }

    #[tokio::test]
    async fn auth_without_phone_fails_if_no_session() {
        let mut client = MockClient::default();
        let result = authenticate(&mut client, None).await;
        assert!(result.is_err());
    }
}
