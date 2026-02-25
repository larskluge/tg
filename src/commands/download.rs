use std::path::PathBuf;

use crate::client::{DownloadOptions, TelegramClient};
use crate::error::Result;
use crate::output::DownloadReport;

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
}
