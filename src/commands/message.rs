use serde::{Deserialize, Serialize};

use crate::cli::MessageArgs;
use crate::client::TelegramClient;
use crate::error::Result;
use crate::output::MessageInfo;

/// Wire mirror of the `message` socket args: one message, named by its chat
/// and its TDLib id, as `download` names one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    pub chat: i64,
    pub message: i64,
}

impl From<MessageArgs> for MessageRequest {
    fn from(args: MessageArgs) -> Self {
        Self {
            chat: args.chat,
            message: args.message,
        }
    }
}

/// Read one message again, wherever it lies in the chat.
///
/// `sync` only returns what is above a cursor or inside a window, and a
/// reaction changes a message far below either without moving its edit date.
/// This is the read a `message_reactions` event asks for: the same
/// `MessageInfo` a listing gives, for exactly the message named. An id that
/// names nothing in the chat is an error, never another message.
pub async fn get_message<C: TelegramClient>(
    client: &C,
    chat_id: i64,
    message_id: i64,
) -> Result<MessageInfo> {
    client.get_message(chat_id, message_id).await
}

pub async fn handle<C: TelegramClient>(client: &C, req: MessageRequest) -> Result<MessageInfo> {
    get_message(client, req.chat, req.message).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;
    use crate::output::{MessageReactionInfo, ReactionSender};

    const GROUP: i64 = -1_009_876_543_210;
    const OLD_MSG: i64 = 918 << 20;
    const NEW_MSG: i64 = 4_211 << 20;

    /// A group with an old message and a much newer one.
    fn mock() -> MockClient {
        let mut client = MockClient::default();
        for id in [OLD_MSG, NEW_MSG] {
            let mut m = client.messages[0].clone();
            m.chat_id = GROUP;
            m.id = id;
            m.text = format!("message {id}");
            client.messages.push(m);
        }
        client
    }

    #[tokio::test]
    async fn handle_returns_the_message_named_however_old() {
        // A cursor at NEW_MSG hides OLD_MSG from every sync. This read does not
        // know about cursors.
        let client = mock();
        let msg = handle(
            &client,
            MessageRequest {
                chat: GROUP,
                message: OLD_MSG,
            },
        )
        .await
        .unwrap();
        assert_eq!(msg.id, OLD_MSG);
        assert_eq!(msg.chat_id, GROUP);
        assert_eq!(msg.text, format!("message {OLD_MSG}"));
    }

    #[tokio::test]
    async fn handle_carries_the_messages_reactions() {
        let client = mock();
        let reactions = vec![MessageReactionInfo {
            emoji: "👍".to_string(),
            count: 2,
            chosen: true,
            recent_senders: vec![ReactionSender {
                id: Some(5_550_101),
                name: Some("Giulia Ferraro".to_string()),
            }],
        }];
        client
            .reactions
            .lock()
            .unwrap()
            .insert((GROUP, OLD_MSG), reactions.clone());

        let msg = get_message(&client, GROUP, OLD_MSG).await.unwrap();
        assert_eq!(msg.reactions, reactions);
        let wire = serde_json::to_value(&msg).unwrap();
        assert_eq!(wire["reactions"][0]["emoji"], "👍");
        assert_eq!(wire["reactions"][0]["chosen"], true);
    }

    #[tokio::test]
    async fn a_message_the_chat_does_not_hold_is_an_error_not_another_message() {
        let client = mock();
        let err = get_message(&client, GROUP, 919 << 20)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not accessible in chat"), "{err}");
        // The id exists, in another chat.
        let err = get_message(&client, 5_550_001, OLD_MSG)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not accessible in chat"), "{err}");
    }

    #[test]
    fn request_reads_the_same_keys_as_download() {
        let req: MessageRequest =
            serde_json::from_str(r#"{"chat": -1009876543210, "message": 962592768}"#).unwrap();
        assert_eq!(req.chat, GROUP);
        assert_eq!(req.message, OLD_MSG);
    }
}
