use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ContactInfo;

pub async fn search_contacts<C: TelegramClient>(
    client: &C,
    query: &str,
) -> Result<Vec<ContactInfo>> {
    client.search_contacts(query).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;

    #[tokio::test]
    async fn search_finds_contacts() {
        let client = MockClient::default();
        let contacts = search_contacts(&client, "John").await.unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].name, "John Doe");
    }

    #[tokio::test]
    async fn search_case_insensitive() {
        let client = MockClient::default();
        let contacts = search_contacts(&client, "john").await.unwrap();
        assert_eq!(contacts.len(), 1);
    }

    #[tokio::test]
    async fn search_no_results() {
        let client = MockClient::default();
        let contacts = search_contacts(&client, "xyz").await.unwrap();
        assert!(contacts.is_empty());
    }
}
