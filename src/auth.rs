use crate::client::TelegramClient;
use crate::error::Result;

pub async fn authenticate<C: TelegramClient>(client: &mut C) -> Result<()> {
    client.authenticate().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn auth_completes_if_code_pending() {
        let mut client = MockClient::with_state(crate::client::mock::AuthState::WaitCode);
        let result = authenticate(&mut client).await;
        assert!(result.is_ok());
        assert!(client.is_authenticated().await);
    }
}
