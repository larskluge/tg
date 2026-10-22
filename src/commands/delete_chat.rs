use serde::{Deserialize, Serialize};

use crate::cli::DeleteChatArgs;
use crate::client::{ChatPresence, TelegramClient, format_timestamp};
use crate::error::{Refusal, Result, TgError};
use crate::output::DeleteChatResult;

/// How far ahead of the clock `through` may lie. It is the date of a message
/// the caller saw, Telegram's, so it is never ahead by more than the clocks
/// differ. One further ahead lets every message that arrives until then pass
/// as not later, and one in milliseconds would let every message pass.
const THROUGH_AHEAD_LIMIT_SECS: i64 = 5 * 60;

/// Wire mirror of the `delete_chat` socket args. Closed like `send`'s and
/// `react`'s (`deny_unknown_fields`): a request that changes a chat refuses an
/// arg it does not know instead of dropping it and deleting anyway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteChatRequest {
    /// The chat's id: for a one-to-one chat, the other person's user id.
    pub id: i64,
    /// The date of the newest message the caller saw in the chat, in Unix
    /// seconds, as Telegram dates a message. A chat whose last message is
    /// later is not deleted.
    pub through: i64,
}

impl From<DeleteChatArgs> for DeleteChatRequest {
    fn from(args: DeleteChatArgs) -> Self {
        Self {
            id: args.id,
            through: args.through,
        }
    }
}

/// Delete a one-to-one chat for this account alone, as Telegram's "Delete
/// chat" does without deleting it for the other person too, unless it holds a
/// message later than `through`.
///
/// Refused, with nothing changed: `not_found` for a chat TDLib does not know,
/// `not_private` for any other kind of chat, `newer_messages` when the chat's
/// last message is later than `through`. A chat that is deleted already
/// answers `deleted` without asking Telegram anything, so a caller may repeat
/// a request whose answer it lost.
pub async fn handle<C: TelegramClient>(
    client: &C,
    req: DeleteChatRequest,
) -> Result<DeleteChatResult> {
    handle_at(client, req, chrono::Utc::now().timestamp()).await
}

async fn handle_at<C: TelegramClient>(
    client: &C,
    req: DeleteChatRequest,
    now: i64,
) -> Result<DeleteChatResult> {
    let DeleteChatRequest {
        id: chat_id,
        through,
    } = req;
    if through > now + THROUGH_AHEAD_LIMIT_SECS {
        return Err(TgError::Other(format!(
            "delete_chat: through {through} lies more than 5 minutes ahead of now ({now}); it is the date of the newest message you saw, in Unix seconds"
        )));
    }

    let chat = read_chat(client, chat_id).await?;
    if !chat.is_private {
        return Err(refused(
            Refusal::NotPrivate,
            format!("chat {chat_id} is no one-to-one chat"),
        ));
    }

    // TDLib asks Telegram to delete up to the last message it knows of, with
    // no bound at all while it knows none, and keeps asking until Telegram
    // answers, after a restart too: such a request, run late, would delete
    // what came in meanwhile. A chat shows no last message for a moment after
    // TDLib loads it from its database, so its history is read from the end,
    // which makes TDLib load it, and the chat is read again.
    let (chat, in_history) = match chat.last_message_date {
        Some(_) => (chat, None),
        None => {
            let newest = newest_in_history(client, chat_id).await?;
            (read_chat(client, chat_id).await?, newest)
        }
    };
    if let Some(date) = chat.last_message_date.max(in_history)
        && i64::from(date) > through
    {
        return Err(refused(
            Refusal::NewerMessages,
            format!(
                "chat {chat_id} has a message of {}, later than through ({})",
                format_timestamp(date),
                format_timestamp(through)
            ),
        ));
    }
    if chat.last_message_date.is_none() {
        // Deleted already, in a Telegram app or by an earlier request whose
        // answer was lost: nothing is left to ask Telegram for.
        if chat.looks_deleted() && in_history.is_none() {
            return Ok(DeleteChatResult::deleted(chat_id));
        }
        return Err(TgError::Other(format!(
            "delete_chat: TDLib knows no last message of chat {chat_id}, and without one it would ask Telegram to delete with no bound, so nothing was deleted"
        )));
    }

    client
        .delete_chat_history(chat_id)
        .await
        .map_err(|e| match e {
            TgError::TdLib(reason) => TgError::Other(format!(
                "delete_chat: Telegram did not delete chat {chat_id}: {reason}"
            )),
            // No refusal: the outcome is not known yet.
            other => TgError::Other(format!("delete_chat: chat {chat_id}: {other}")),
        })?;
    Ok(DeleteChatResult::deleted(chat_id))
}

/// How long the history read may take. It goes to Telegram when TDLib does
/// not hold the chat's newest messages, and `tg serve` holds its client lock
/// meanwhile; with the delete's own 20 s a request is answered within about
/// 30 s.
const HISTORY_READ_TIMEOUT_SECS: u64 = 8;

/// The chat as TDLib holds it, or `not_found`.
async fn read_chat<C: TelegramClient>(client: &C, chat_id: i64) -> Result<ChatPresence> {
    client
        .find_chat_presence(chat_id)
        .await
        .map_err(|e| TgError::Other(format!("delete_chat: reading chat {chat_id} failed: {e}")))?
        .ok_or_else(|| {
            refused(
                Refusal::NotFound,
                format!("Telegram's client here does not know chat {chat_id}"),
            )
        })
}

/// The date of the chat's newest message, read from its end, or `None` for a
/// chat with no messages.
async fn newest_in_history<C: TelegramClient>(client: &C, chat_id: i64) -> Result<Option<i32>> {
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(HISTORY_READ_TIMEOUT_SECS),
        client.get_messages(chat_id, 1, None),
    )
    .await
    .map_err(|_| {
        TgError::Other(format!(
            "delete_chat: reading the last message of chat {chat_id} did not finish within {HISTORY_READ_TIMEOUT_SECS}s, so nothing was deleted"
        ))
    })?
    .map_err(|e| {
        TgError::Other(format!(
            "delete_chat: reading the last message of chat {chat_id} failed, so nothing was deleted: {e}"
        ))
    })?;
    Ok(read.iter().map(|m| m.timestamp).max())
}

fn refused(refusal: Refusal, why: String) -> TgError {
    TgError::Refused {
        refusal,
        message: format!("delete_chat: {why}, so nothing was deleted"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::MockClient;
    use crate::output::MessageInfo;

    const NOW: i64 = 1_800_000_000;
    /// A one-to-one chat, by its peer's (fictional) user id.
    const PEER: i64 = 555_000_111;

    fn client_with(chat_id: i64, chat: ChatPresence) -> MockClient {
        let client = MockClient::default();
        client.known_chats.lock().unwrap().insert(chat_id, chat);
        client
    }

    /// A listed one-to-one chat.
    fn private(last_message_date: Option<i32>) -> ChatPresence {
        ChatPresence {
            is_private: true,
            in_chat_list: true,
            last_message_date,
        }
    }

    /// A one-to-one chat as a delete leaves it: in no list, no last message.
    fn gone() -> ChatPresence {
        ChatPresence {
            is_private: true,
            in_chat_list: false,
            last_message_date: None,
        }
    }

    fn message_at(timestamp: i32) -> MessageInfo {
        MessageInfo {
            id: 7 << 20,
            chat_id: PEER,
            sender_id: Some(PEER),
            sender: "Ada Lindqvist".to_string(),
            sender_is_bot: Some(false),
            text: "See you then".to_string(),
            date: "-".to_string(),
            timestamp,
            is_outgoing: false,
            edit_date: None,
            content_type: Some("text".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: None,
            reply_to_message_id: None,
            reactions: vec![],
        }
    }

    fn request(through: i64) -> DeleteChatRequest {
        DeleteChatRequest { id: PEER, through }
    }

    fn deleted(client: &MockClient) -> Vec<i64> {
        client.deleted_chats.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn deletes_a_chat_whose_last_message_is_the_one_seen() {
        let seen = (NOW - 600) as i32;
        let client = client_with(PEER, private(Some(seen)));
        let result = handle_at(&client, request(i64::from(seen)), NOW)
            .await
            .unwrap();
        assert_eq!(result, DeleteChatResult::deleted(PEER));
        assert_eq!(result.code, "deleted");
        assert_eq!(deleted(&client), vec![PEER]);
    }

    #[tokio::test]
    async fn a_later_message_refuses_it_and_deletes_nothing() {
        let client = client_with(PEER, private(Some((NOW - 60) as i32)));
        let err = handle_at(&client, request(NOW - 61), NOW)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("newer_messages"));
        let text = err.to_string();
        assert!(text.contains("later than through"), "{text}");
        assert!(text.contains("nothing was deleted"), "{text}");
        assert!(deleted(&client).is_empty());
    }

    #[tokio::test]
    async fn an_unknown_chat_is_not_found() {
        let client = MockClient::default();
        let err = handle_at(&client, request(NOW), NOW).await.unwrap_err();
        assert_eq!(err.code(), Some("not_found"));
        assert!(err.to_string().contains(&PEER.to_string()));
        assert!(deleted(&client).is_empty());
    }

    #[tokio::test]
    async fn a_group_is_refused() {
        let group = -1_009_876_543_210;
        let client = client_with(
            group,
            ChatPresence {
                is_private: false,
                in_chat_list: true,
                last_message_date: Some((NOW - 3600) as i32),
            },
        );
        let err = handle_at(
            &client,
            DeleteChatRequest {
                id: group,
                through: NOW,
            },
            NOW,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), Some("not_private"));
        assert!(deleted(&client).is_empty());
    }

    #[tokio::test]
    async fn without_a_last_message_the_history_is_read_first() {
        // TDLib would ask Telegram to delete with no bound, so the history is
        // read from its end, which gives the chat its last message.
        let client = MockClient {
            messages: vec![message_at((NOW - 30) as i32)],
            ..client_with(PEER, private(None))
        };
        let err = handle_at(&client, request(NOW - 31), NOW)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("newer_messages"));
        assert!(deleted(&client).is_empty());

        let result = handle_at(&client, request(NOW - 30), NOW).await.unwrap();
        assert_eq!(result, DeleteChatResult::deleted(PEER));
        assert_eq!(deleted(&client), vec![PEER]);
    }

    #[tokio::test]
    async fn the_delete_waits_for_tdlib_to_know_the_last_message() {
        // The history read alone says how new the chat is; only the chat read
        // again says that TDLib now bounds the delete by it.
        let client = MockClient {
            messages: vec![message_at((NOW - 30) as i32)],
            ..client_with(PEER, private(None))
        };
        let result = handle_at(&client, request(NOW - 30), NOW).await.unwrap();
        assert_eq!(result, DeleteChatResult::deleted(PEER));
        assert_eq!(deleted(&client), vec![PEER]);
    }

    #[tokio::test]
    async fn a_message_in_the_history_alone_does_not_bound_the_delete() {
        // The history read found a message no later than `through`, but TDLib
        // still shows no last message, so it would delete with no bound.
        let client = MockClient {
            messages: vec![message_at((NOW - 30) as i32)],
            history_leaves_no_last_message: true,
            ..client_with(PEER, gone())
        };
        let err = handle_at(&client, request(NOW - 30), NOW)
            .await
            .unwrap_err();
        assert_eq!(err.code(), None);
        assert!(err.to_string().contains("no bound"), "{err}");
        assert!(deleted(&client).is_empty());
    }

    #[tokio::test]
    async fn a_listed_chat_whose_last_message_stays_unknown_is_not_deleted() {
        // The one way left to ask Telegram for a delete with no bound: TDLib
        // lists the chat, reads no message from its end, and still shows none.
        let client = MockClient {
            messages: vec![],
            ..client_with(PEER, private(None))
        };
        let err = handle_at(&client, request(NOW), NOW).await.unwrap_err();
        assert_eq!(err.code(), None);
        assert!(err.to_string().contains("no bound"), "{err}");
        assert!(deleted(&client).is_empty());
    }

    #[tokio::test]
    async fn a_chat_deleted_already_answers_deleted_and_asks_telegram_nothing() {
        // A caller that lost the first answer repeats the request, or the chat
        // was deleted in a Telegram app.
        let client = MockClient {
            messages: vec![],
            ..client_with(PEER, private(Some((NOW - 600) as i32)))
        };
        handle_at(&client, request(NOW - 600), NOW).await.unwrap();
        let again = handle_at(&client, request(NOW - 600), NOW).await.unwrap();
        assert_eq!(again, DeleteChatResult::deleted(PEER));
        assert_eq!(deleted(&client), vec![PEER], "asked once");

        let elsewhere = MockClient {
            messages: vec![],
            ..client_with(PEER, gone())
        };
        let result = handle_at(&elsewhere, request(NOW), NOW).await.unwrap();
        assert_eq!(result, DeleteChatResult::deleted(PEER));
        assert!(deleted(&elsewhere).is_empty());
    }

    #[tokio::test]
    async fn telegrams_refusal_is_an_error_with_no_code() {
        let client = MockClient {
            delete_refusal: Some("Chat history can't be deleted".to_string()),
            ..client_with(PEER, private(Some((NOW - 600) as i32)))
        };
        let err = handle_at(&client, request(NOW), NOW).await.unwrap_err();
        assert_eq!(err.code(), None);
        let text = err.to_string();
        assert!(
            text.contains(&format!("did not delete chat {PEER}")),
            "{text}"
        );
        assert!(text.contains("Chat history can't be deleted"), "{text}");
        assert!(
            !text.contains("TDLib error"),
            "Telegram's reason alone: {text}"
        );
    }

    #[tokio::test]
    async fn an_unconfirmed_delete_does_not_say_it_failed() {
        // TDLib has deleted the chat on its side and keeps asking Telegram, so
        // the answer says the outcome is open, and a repeat answers deleted.
        let client = MockClient {
            messages: vec![],
            delete_unconfirmed: true,
            ..client_with(PEER, private(Some((NOW - 600) as i32)))
        };
        let err = handle_at(&client, request(NOW - 600), NOW)
            .await
            .unwrap_err();
        assert_eq!(err.code(), None);
        let text = err.to_string();
        assert!(!text.contains("did not delete"), "{text}");
        assert!(text.contains("not confirmed"), "{text}");
        let again = handle_at(&client, request(NOW - 600), NOW).await.unwrap();
        assert_eq!(again, DeleteChatResult::deleted(PEER));
        assert_eq!(deleted(&client), vec![PEER]);
    }

    #[tokio::test]
    async fn a_chat_that_cannot_be_read_is_an_error_not_a_missing_chat() {
        let client = MockClient {
            find_chat_error: Some("Invalid chat identifier specified".to_string()),
            ..MockClient::default()
        };
        let err = handle_at(&client, request(NOW), NOW).await.unwrap_err();
        assert_eq!(err.code(), None, "{err}");
        assert!(err.to_string().contains("Invalid chat identifier"));
        assert!(deleted(&client).is_empty());
    }

    #[tokio::test]
    async fn a_through_ahead_of_the_clock_is_refused_before_telegram_is_asked() {
        // In milliseconds no message would ever be later, and the bound would
        // guard nothing. The chat cannot be read here, so an answer about the
        // bound proves the chat was never asked about.
        let client = MockClient {
            find_chat_error: Some("must not be asked".to_string()),
            ..MockClient::default()
        };
        for through in [NOW * 1000, NOW + 3600] {
            let err = handle_at(&client, request(through), NOW).await.unwrap_err();
            assert_eq!(err.code(), None);
            assert!(err.to_string().contains("Unix seconds"), "{err}");
        }
        assert!(deleted(&client).is_empty());
        // A clock a minute behind Telegram's is no reason to refuse.
        let behind = client_with(PEER, private(Some(NOW as i32 + 60)));
        let result = handle_at(&behind, request(NOW + 60), NOW).await.unwrap();
        assert_eq!(result, DeleteChatResult::deleted(PEER));
    }

    #[test]
    fn the_request_is_closed() {
        let err = serde_json::from_value::<DeleteChatRequest>(serde_json::json!({
            "id": PEER, "through": NOW, "revoke": true
        }))
        .unwrap_err();
        assert!(err.to_string().contains("unknown field `revoke`"), "{err}");
        assert!(
            serde_json::from_value::<DeleteChatRequest>(serde_json::json!({"id": PEER})).is_err(),
            "through is required"
        );
    }
}
