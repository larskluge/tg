use serde::{Deserialize, Serialize};

use crate::cli::ReactArgs;
use crate::client::TelegramClient;
use crate::commands::send::validate_message_id;
use crate::error::{Result, TgError};
use crate::output::{MessageInfo, ReactResult};
use crate::reactions::{emoji_key, telegram_form};

/// Wire mirror of the `react` socket args. Closed like `send`'s
/// (`deny_unknown_fields`), and for the same reason: an arg `tg` does not know
/// must be refused, not dropped while the reaction still goes out.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReactRequest {
    /// The chat's id.
    pub id: i64,
    /// The TDLib id of the message in that chat: `server_id << 20`, as
    /// `tg messages` and `tg sync` print it.
    pub message_id: i64,
    /// The emoji to react with, or to take back. With or without a variation
    /// selector: Telegram's own spelling is found before it is sent. Empty
    /// only with `remove`, where it means every reaction of the account.
    #[serde(default)]
    pub emoji: String,
    /// Take the reaction back instead of adding it.
    #[serde(default)]
    pub remove: bool,
}

impl From<ReactArgs> for ReactRequest {
    fn from(args: ReactArgs) -> Self {
        Self {
            id: args.chat,
            message_id: args.message,
            emoji: args.emoji.unwrap_or_default(),
            remove: args.remove,
        }
    }
}

/// React to one message, or take a reaction back, and answer what the message
/// shows afterwards.
///
/// The order is `send`'s: the request's shape first, then the proof that the
/// message is in that chat, and only then the call that changes something.
/// Success always means the message was read back and shows what was asked
/// for; a read-back that disagrees is an error, never `chosen` with the other
/// value.
pub async fn handle<C: TelegramClient>(client: &C, req: ReactRequest) -> Result<ReactResult> {
    let chat_id = req.id;
    let message_id = validate_message_id("message_id", req.message_id)?;
    if !req.remove && req.emoji.is_empty() {
        return Err(TgError::Other(
            "react: emoji is required (it may be empty only with remove, which then takes back every reaction of the account)"
                .to_string(),
        ));
    }

    let before = client
        .get_message(chat_id, message_id)
        .await
        .map_err(|e| TgError::Other(format!("react: {e}")))?;

    let target = Target {
        chat_id,
        message_id,
    };
    if req.remove {
        take_back(client, target, &req.emoji, &before).await
    } else {
        add(client, target, &req.emoji).await
    }
}

/// The message a reaction is for.
#[derive(Clone, Copy)]
struct Target {
    chat_id: i64,
    message_id: i64,
}

impl Target {
    /// What Telegram said no to, with its own reason. The emoji is left out:
    /// a caller logs this line.
    fn refused(self, e: TgError) -> TgError {
        let Self {
            chat_id,
            message_id,
        } = self;
        match e {
            TgError::TdLib(reason) => TgError::Other(format!(
                "react: Telegram refused the reaction to message {message_id} in chat {chat_id}: {reason}"
            )),
            other => TgError::Other(format!("react: {other}")),
        }
    }

    /// The message as it is after Telegram answered.
    async fn read_back<C: TelegramClient>(self, client: &C, did: &str) -> Result<MessageInfo> {
        let Self {
            chat_id,
            message_id,
        } = self;
        client.get_message(chat_id, message_id).await.map_err(|e| {
            TgError::Other(format!(
                "react: Telegram {did}, but reading message {message_id} in chat {chat_id} back failed: {e}"
            ))
        })
    }
}

/// The account's reactions on `message` that are `emoji` in either spelling,
/// or all of them for an empty `emoji`.
fn chosen<'a>(message: &'a MessageInfo, emoji: &str) -> Vec<&'a str> {
    let key = emoji_key(emoji);
    message
        .reactions
        .iter()
        .filter(|r| r.chosen && (emoji.is_empty() || emoji_key(&r.emoji) == key))
        .map(|r| r.emoji.as_str())
        .collect()
}

async fn add<C: TelegramClient>(client: &C, target: Target, asked: &str) -> Result<ReactResult> {
    let Target {
        chat_id,
        message_id,
    } = target;

    // TDLib compares a reaction to the ones Telegram offers byte for byte, so
    // a keyboard's "❤️" is refused where Telegram offers "❤". An emoji Telegram
    // offers in no spelling goes out as given: the refusal is then Telegram's
    // own, with its own reason.
    let offered = client
        .get_available_reactions(chat_id, message_id)
        .await
        .map_err(|e| TgError::Other(format!("react: {e}")))?;
    let emoji = telegram_form(asked, &offered).unwrap_or_else(|| asked.to_string());

    client
        .add_message_reaction(chat_id, message_id, &emoji)
        .await
        .map_err(|e| target.refused(e))?;

    let after = target.read_back(client, "accepted the reaction").await?;
    match chosen(&after, &emoji).first() {
        Some(shown) => Ok(ReactResult {
            chat_id,
            message_id,
            emoji: shown.to_string(),
            chosen: true,
        }),
        None => Err(TgError::Other(format!(
            "react: Telegram accepted the reaction, but message {message_id} in chat {chat_id} does not show it as the account's"
        ))),
    }
}

async fn take_back<C: TelegramClient>(
    client: &C,
    target: Target,
    asked: &str,
    before: &MessageInfo,
) -> Result<ReactResult> {
    let Target {
        chat_id,
        message_id,
    } = target;

    let mine = chosen(before, asked);
    for emoji in &mine {
        client
            .remove_message_reaction(chat_id, message_id, emoji)
            .await
            .map_err(|e| target.refused(e))?;
    }
    // Nothing of the account's there is nothing to take back: the answer is
    // the same "not chosen", with no call made.
    if !mine.is_empty() {
        let after = target.read_back(client, "accepted the removal").await?;
        if !chosen(&after, asked).is_empty() {
            return Err(TgError::Other(format!(
                "react: Telegram accepted the removal, but message {message_id} in chat {chat_id} still shows the reaction as the account's"
            )));
        }
    }

    Ok(ReactResult {
        chat_id,
        message_id,
        emoji: match (asked.is_empty(), mine.first()) {
            (true, _) => String::new(),
            (false, Some(shown)) => shown.to_string(),
            (false, None) => asked.to_string(),
        },
        chosen: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::mock::{MockClient, ReactionCall};
    use crate::output::{MessageReactionInfo, ReactionSender};
    use serde_json::json;

    // Fictional TDLib ids, as in `send`'s reply tests: a DM and a supergroup,
    // each holding one message.
    const DM: i64 = 5_550_001;
    const GROUP: i64 = -1_009_876_543_210;
    const DM_MSG: i64 = 4_211 << 20;
    const GROUP_MSG: i64 = 918 << 20;

    const HEART: &str = "\u{2764}";
    const HEART_WITH_SELECTOR: &str = "\u{2764}\u{FE0F}";

    fn mock() -> MockClient {
        let mut client = MockClient::default();
        for (chat_id, id) in [(DM, DM_MSG), (GROUP, GROUP_MSG)] {
            let mut m = client.messages[0].clone();
            m.chat_id = chat_id;
            m.id = id;
            client.messages.push(m);
        }
        client
    }

    fn others(emoji: &str, count: i32, chosen: bool) -> MessageReactionInfo {
        MessageReactionInfo {
            emoji: emoji.to_string(),
            count,
            chosen,
            recent_senders: vec![ReactionSender {
                id: Some(5_550_101),
                name: Some("Giulia Ferraro".to_string()),
            }],
        }
    }

    /// A client whose group message already carries these reactions.
    fn client_with(reactions: Vec<MessageReactionInfo>) -> MockClient {
        let client = mock();
        client
            .reactions
            .lock()
            .unwrap()
            .insert((GROUP, GROUP_MSG), reactions);
        client
    }

    fn request(emoji: &str, remove: bool) -> ReactRequest {
        ReactRequest {
            id: GROUP,
            message_id: GROUP_MSG,
            emoji: emoji.to_string(),
            remove,
        }
    }

    fn react_req(v: serde_json::Value) -> serde_json::Result<ReactRequest> {
        serde_json::from_value(v)
    }

    fn calls(client: &MockClient) -> Vec<ReactionCall> {
        client.reaction_calls.lock().unwrap().clone()
    }

    #[test]
    fn react_request_reads_the_documented_args() {
        let req = react_req(json!({"id": GROUP, "message_id": GROUP_MSG, "emoji": "👍"})).unwrap();
        assert_eq!(req.id, GROUP);
        assert_eq!(req.message_id, GROUP_MSG);
        assert_eq!(req.emoji, "👍");
        assert!(!req.remove, "remove defaults to false");

        let req = react_req(json!({"id": GROUP, "message_id": GROUP_MSG, "remove": true})).unwrap();
        assert!(req.remove);
        assert_eq!(req.emoji, "", "taking back needs no emoji");
    }

    #[test]
    fn react_request_rejects_an_unknown_field() {
        let err = react_req(json!({
            "id": GROUP, "message_id": GROUP_MSG, "emoji": "👍", "big": true
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field `big`"), "{err}");
        for field in ["id", "message_id", "emoji", "remove"] {
            assert!(err.contains(field), "error should name `{field}`: {err}");
        }
    }

    #[test]
    fn react_request_requires_the_chat_and_the_message() {
        let err = react_req(json!({"emoji": "👍"})).unwrap_err().to_string();
        assert!(err.contains("missing field `id`"), "{err}");
        let err = react_req(json!({"id": GROUP, "emoji": "👍"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing field `message_id`"), "{err}");
    }

    #[tokio::test]
    async fn handle_validates_the_message_id_before_touching_the_client() {
        // A bare server id (what a t.me link shows), zero and a negative number
        // are all a caller holding the wrong number.
        for bad in [918, 0, -(918 << 20)] {
            let client = mock();
            let req = ReactRequest {
                message_id: bad,
                ..request("👍", false)
            };
            let err = handle(&client, req).await.unwrap_err().to_string();
            assert!(err.contains("invalid message_id"), "{bad}: {err}");
            assert_eq!(
                client
                    .get_message_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            assert!(calls(&client).is_empty());
        }
    }

    #[tokio::test]
    async fn handle_refuses_to_add_without_an_emoji() {
        let client = mock();
        let err = handle(&client, request("", false))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("emoji is required"), "{err}");
        assert!(calls(&client).is_empty());
    }

    #[tokio::test]
    async fn handle_reacts_and_answers_what_the_message_shows() {
        let client = mock();
        let res = handle(&client, request("👍", false)).await.unwrap();
        assert_eq!(
            res,
            ReactResult {
                chat_id: GROUP,
                message_id: GROUP_MSG,
                emoji: "👍".to_string(),
                chosen: true,
            }
        );
        assert_eq!(
            calls(&client),
            vec![ReactionCall::Added(GROUP, GROUP_MSG, "👍".to_string())]
        );
    }

    #[tokio::test]
    async fn handle_sends_telegrams_spelling_of_the_emoji() {
        // The mock, like TDLib, takes only the bytes Telegram offers: the bare
        // heart. A keyboard's heart with the selector must still arrive.
        let client = mock();
        let res = handle(&client, request(HEART_WITH_SELECTOR, false))
            .await
            .unwrap();
        assert_eq!(
            calls(&client),
            vec![ReactionCall::Added(GROUP, GROUP_MSG, HEART.to_string())]
        );
        assert_eq!(res.emoji, HEART);
        assert!(res.chosen);
    }

    #[tokio::test]
    async fn handle_passes_telegrams_refusal_through() {
        // The chat does not offer a unicorn. It is sent as asked, so the reason
        // is Telegram's own, and the caller's log line names no emoji.
        let client = mock();
        let err = handle(&client, request("🦄", false))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!(
                "react: Telegram refused the reaction to message {GROUP_MSG} in chat {GROUP}: \
                 The reaction isn't available for the message"
            )
        );
        assert!(client.reactions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_refuses_a_message_the_chat_does_not_hold() {
        // DM_MSG is a real message, in the DM. The same number in the group
        // names nothing, and nothing is sent.
        let client = mock();
        let req = ReactRequest {
            message_id: DM_MSG,
            ..request("👍", false)
        };
        let err = handle(&client, req).await.unwrap_err().to_string();
        assert!(err.starts_with("react: "), "{err}");
        assert!(err.contains("not accessible in chat"), "{err}");
        assert!(calls(&client).is_empty());
    }

    #[tokio::test]
    async fn another_emoji_replaces_the_accounts_reaction_and_leaves_the_others() {
        let client = client_with(vec![others("👍", 2, true), others("🔥", 1, false)]);
        let res = handle(&client, request("🔥", false)).await.unwrap();
        assert_eq!(res.emoji, "🔥");
        assert!(res.chosen);

        let now = client.reactions.lock().unwrap()[&(GROUP, GROUP_MSG)].clone();
        let thumbs = now.iter().find(|r| r.emoji == "👍").unwrap();
        assert!(!thumbs.chosen);
        assert_eq!(thumbs.count, 1, "Giulia's thumbs-up stays");
        let fire = now.iter().find(|r| r.emoji == "🔥").unwrap();
        assert!(fire.chosen);
        assert_eq!(fire.count, 2);
    }

    #[tokio::test]
    async fn handle_takes_a_reaction_back() {
        let client = client_with(vec![others("👍", 2, true)]);
        let res = handle(&client, request("👍", true)).await.unwrap();
        assert_eq!(
            res,
            ReactResult {
                chat_id: GROUP,
                message_id: GROUP_MSG,
                emoji: "👍".to_string(),
                chosen: false,
            }
        );
        assert_eq!(
            calls(&client),
            vec![ReactionCall::Removed(GROUP, GROUP_MSG, "👍".to_string())]
        );
    }

    #[tokio::test]
    async fn handle_takes_back_by_either_spelling() {
        let client = client_with(vec![others(HEART, 1, true)]);
        let res = handle(&client, request(HEART_WITH_SELECTOR, true))
            .await
            .unwrap();
        assert_eq!(
            calls(&client),
            vec![ReactionCall::Removed(GROUP, GROUP_MSG, HEART.to_string())]
        );
        assert_eq!(res.emoji, HEART);
        assert!(!res.chosen);
    }

    #[tokio::test]
    async fn taking_back_without_an_emoji_takes_back_whatever_the_account_has() {
        let client = client_with(vec![others("👍", 3, false), others(HEART, 2, true)]);
        let res = handle(&client, request("", true)).await.unwrap();
        assert_eq!(
            calls(&client),
            vec![ReactionCall::Removed(GROUP, GROUP_MSG, HEART.to_string())]
        );
        assert_eq!(res.emoji, "");
        assert!(!res.chosen);
    }

    #[tokio::test]
    async fn taking_back_a_reaction_that_is_not_there_changes_nothing() {
        // The account's reaction is a heart. Asked to take back a thumbs-up it
        // never gave, nothing is sent, and "not chosen" is still true.
        let client = client_with(vec![others("👍", 3, false), others(HEART, 2, true)]);
        let res = handle(&client, request("👍", true)).await.unwrap();
        assert!(calls(&client).is_empty());
        assert_eq!(res.emoji, "👍");
        assert!(!res.chosen);

        // And on a message with no reactions at all.
        let client = mock();
        let res = handle(&client, request("", true)).await.unwrap();
        assert!(calls(&client).is_empty());
        assert!(!res.chosen);
    }

    #[tokio::test]
    async fn a_reaction_the_message_does_not_show_afterwards_is_an_error() {
        let mut client = mock();
        client.reactions_take_no_effect = true;
        let err = handle(&client, request("👍", false))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("accepted the reaction"), "{err}");
        assert!(err.contains("does not show it"), "{err}");
    }

    #[tokio::test]
    async fn a_removal_the_message_still_shows_afterwards_is_an_error() {
        let mut client = client_with(vec![others("👍", 2, true)]);
        client.reactions_take_no_effect = true;
        let err = handle(&client, request("👍", true))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("accepted the removal"), "{err}");
        assert!(err.contains("still shows"), "{err}");
    }

    #[tokio::test]
    async fn telegrams_refusal_of_a_removal_is_passed_through_too() {
        let mut client = client_with(vec![others("👍", 2, true)]);
        client.reaction_refusal = Some("MESSAGE_NOT_MODIFIED".to_string());
        let err = handle(&client, request("👍", true))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Telegram refused the reaction"), "{err}");
        assert!(err.ends_with("MESSAGE_NOT_MODIFIED"), "{err}");
    }

    #[test]
    fn the_cli_args_become_the_same_request() {
        use crate::cli::{Cli, Command};
        use clap::Parser;

        let cli = Cli::parse_from([
            "tg",
            "react",
            "--chat",
            "-1009876543210",
            "--message",
            "962592768",
            "👍",
        ]);
        let Command::React(args) = cli.command else {
            panic!("expected react");
        };
        let req = ReactRequest::from(args);
        assert_eq!(req.id, GROUP);
        assert_eq!(req.message_id, GROUP_MSG);
        assert_eq!(req.emoji, "👍");
        assert!(!req.remove);
    }
}
