use serde::{Deserialize, Serialize};

use crate::cli::SearchArgs;
use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::ContactInfo;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
}

impl From<SearchArgs> for SearchRequest {
    fn from(args: SearchArgs) -> Self {
        Self { query: args.query }
    }
}

pub async fn search_contacts<C: TelegramClient>(
    client: &C,
    query: &str,
) -> Result<Vec<ContactInfo>> {
    client.search_contacts(query).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: SearchRequest) -> Result<Vec<ContactInfo>> {
    search_contacts(client, &req.query).await
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

    #[tokio::test]
    async fn handle_runs_search() {
        let client = MockClient::default();
        let req = SearchRequest {
            query: "John".to_string(),
        };
        let contacts = handle(&client, req).await.unwrap();
        assert_eq!(contacts.len(), 1);
    }
}
