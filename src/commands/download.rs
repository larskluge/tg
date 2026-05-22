use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cli::DownloadArgs;
use crate::client::{DownloadOptions, TelegramClient};
use crate::error::Result;
use crate::output::DownloadReport;

fn default_output_dir() -> PathBuf {
    PathBuf::from(".")
}

fn default_priority() -> i32 {
    16
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub chat: i64,
    pub message: i64,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_priority")]
    pub priority: i32,
}

impl From<DownloadArgs> for DownloadRequest {
    fn from(args: DownloadArgs) -> Self {
        Self {
            chat: args.chat,
            message: args.message,
            output_dir: args.output_dir,
            priority: args.priority,
        }
    }
}

pub async fn download_message_media<C: TelegramClient>(
    client: &C,
    chat_id: i64,
    message_id: i64,
    output_dir: PathBuf,
    priority: i32,
) -> Result<DownloadReport> {
    let options = DownloadOptions {
        output_dir,
        priority,
    };
    client
        .download_message_media(chat_id, message_id, options)
        .await
}

pub async fn handle<C: TelegramClient>(client: &C, req: DownloadRequest) -> Result<DownloadReport> {
    download_message_media(client, req.chat, req.message, req.output_dir, req.priority).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;
    use crate::output::DownloadStatus;

    #[tokio::test]
    async fn download_message_calls_client() {
        let client = MockClient::default();
        let report = download_message_media(&client, 1, 2, PathBuf::from("."), 16)
            .await
            .unwrap();
        assert_eq!(report.chat_id, 1);
        assert_eq!(report.message_id, 2);
        assert_eq!(report.status, DownloadStatus::NoDownloadableMedia);
    }

    #[tokio::test]
    async fn handle_calls_client_with_fields() {
        let client = MockClient::default();
        let req = DownloadRequest {
            chat: 7,
            message: 8,
            output_dir: PathBuf::from("/tmp"),
            priority: 16,
        };
        let report = handle(&client, req).await.unwrap();
        assert_eq!(report.chat_id, 7);
        assert_eq!(report.message_id, 8);
    }

    #[test]
    fn request_deserializes_with_defaults() {
        let req: DownloadRequest = serde_json::from_str(r#"{"chat":1,"message":2}"#).unwrap();
        assert_eq!(req.output_dir, PathBuf::from("."));
        assert_eq!(req.priority, 16);
    }
}
