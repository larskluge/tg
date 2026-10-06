//! Reactions: how TDLib's `messageReactions` becomes the list a message
//! carries, and which spelling of an emoji Telegram takes.
//!
//! Everything here is a pure function over TDLib's own types, so it is tested
//! on the JSON TDLib sends. The calls that need a client (`getUser` for a name,
//! `getCustomEmojiStickers`, `addMessageReaction`) live in `client.rs`.

use std::collections::HashMap;

use tdlib_rs::enums::{MessageSender, ReactionType, StickerFullType};
use tdlib_rs::types::{MessageInteractionInfo, MessageReaction, Sticker};

/// The variation selector that asks for an emoji's coloured presentation.
/// Telegram's reactions never carry it (its heart is the bare U+2764), while
/// a keyboard's "❤️" does.
const EMOJI_PRESENTATION: char = '\u{FE0F}';

/// An emoji without its variation selectors: the form in which two spellings
/// of one emoji compare equal. For comparing only, never for sending.
pub fn emoji_key(emoji: &str) -> String {
    emoji.chars().filter(|c| *c != EMOJI_PRESENTATION).collect()
}

/// The standard emoji each custom emoji stands for, by custom emoji id.
pub type CustomEmoji = HashMap<i64, String>;

/// One emoji on a message, before its senders have names.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedReaction {
    pub emoji: String,
    pub count: i32,
    pub chosen: bool,
    /// The other people TDLib names, never the account itself.
    pub senders: Vec<MessageSender>,
    /// The TDLib reactions of the account that are listed under this emoji:
    /// what `removeMessageReaction` has to be given to take it back. More than
    /// one when a custom emoji joins the standard emoji it stands for.
    pub chosen_types: Vec<ReactionType>,
}

/// The custom emoji a message's reactions use, each id once.
pub fn custom_emoji_ids(info: Option<&MessageInteractionInfo>) -> Vec<i64> {
    let mut ids = Vec::new();
    for reaction in tdlib_reactions(info) {
        if let ReactionType::CustomEmoji(custom) = &reaction.r#type
            && !ids.contains(&custom.custom_emoji_id)
        {
            ids.push(custom.custom_emoji_id);
        }
    }
    ids
}

/// The `messageReaction`s TDLib holds for a message, or none.
fn tdlib_reactions(info: Option<&MessageInteractionInfo>) -> &[MessageReaction] {
    info.and_then(|i| i.reactions.as_ref())
        .map_or(&[], |r| r.reactions.as_slice())
}

/// What `getCustomEmojiStickers` answered, as the emoji each one stands for.
/// A sticker that names none is left out.
pub fn custom_emoji_names(stickers: &[Sticker]) -> CustomEmoji {
    stickers
        .iter()
        .filter_map(|sticker| match &sticker.full_type {
            StickerFullType::CustomEmoji(custom) if !sticker.emoji.is_empty() => {
                Some((custom.custom_emoji_id, sticker.emoji.clone()))
            }
            _ => None,
        })
        .collect()
}

/// A message's reactions, one entry per emoji, in TDLib's order.
///
/// - A paid reaction is left out.
/// - A custom emoji is listed under the standard emoji `custom` names for it
///   and joins that emoji's own entry when the message has one; a custom emoji
///   `custom` does not name is left out.
/// - The account is never among `senders`: `chosen` says it reacted. TDLib
///   sets `used_sender_id` exactly when the account's own sender is one of the
///   recent ones it names, so that is the one dropped.
pub fn listed_reactions(
    info: Option<&MessageInteractionInfo>,
    custom: &CustomEmoji,
) -> Vec<ListedReaction> {
    let mut listed: Vec<ListedReaction> = Vec::new();
    for reaction in tdlib_reactions(info) {
        let (emoji, is_standard) = match &reaction.r#type {
            ReactionType::Emoji(e) => (e.emoji.as_str(), true),
            ReactionType::CustomEmoji(c) => match custom.get(&c.custom_emoji_id) {
                Some(stands_for) => (stands_for.as_str(), false),
                None => continue,
            },
            ReactionType::Paid => continue,
        };
        let others = reaction
            .recent_sender_ids
            .iter()
            .filter(|sender| reaction.used_sender_id.as_ref() != Some(*sender));

        let key = emoji_key(emoji);
        let position = listed.iter().position(|l| emoji_key(&l.emoji) == key);
        let entry = match position {
            Some(position) => &mut listed[position],
            None => {
                listed.push(ListedReaction {
                    emoji: emoji.to_string(),
                    count: 0,
                    chosen: false,
                    senders: Vec::new(),
                    chosen_types: Vec::new(),
                });
                listed.last_mut().expect("just pushed")
            }
        };
        if is_standard {
            // Telegram's own spelling wins over a sticker's.
            entry.emoji = emoji.to_string();
        }
        entry.count += reaction.total_count;
        entry.chosen |= reaction.is_chosen;
        for sender in others {
            if !entry.senders.contains(sender) {
                entry.senders.push(sender.clone());
            }
        }
        if reaction.is_chosen {
            entry.chosen_types.push(reaction.r#type.clone());
        }
    }
    listed
}

/// The TDLib reactions of the account that a message lists as `emoji`, in
/// either spelling: what to hand `removeMessageReaction` to take it back.
pub fn chosen_types(
    info: Option<&MessageInteractionInfo>,
    custom: &CustomEmoji,
    emoji: &str,
) -> Vec<ReactionType> {
    let key = emoji_key(emoji);
    listed_reactions(info, custom)
        .into_iter()
        .filter(|listed| emoji_key(&listed.emoji) == key)
        .flat_map(|listed| listed.chosen_types)
        .collect()
}

/// The spelling of `asked` that Telegram offers for a message, given every
/// emoji it offers there. `None` when it offers that emoji in no spelling.
///
/// TDLib compares a reaction to the offered ones byte for byte, so "❤️" is
/// refused where "❤" is offered. An exact match stands; otherwise the one
/// offered emoji that differs only by variation selectors is the answer.
pub fn telegram_form(asked: &str, offered: &[String]) -> Option<String> {
    if asked.is_empty() {
        return None;
    }
    if offered.iter().any(|o| o == asked) {
        return Some(asked.to_string());
    }
    let key = emoji_key(asked);
    let mut spellings = offered.iter().filter(|o| emoji_key(o) == key);
    match (spellings.next(), spellings.next()) {
        (Some(only), None) => Some(only.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Lars's own user id in these fixtures, and three other people.
    const ME: i64 = 5_550_001;
    const GIULIA: i64 = 5_550_101;
    const PAOLO: i64 = 5_550_102;
    const MARCO: i64 = 5_550_103;
    const CHANNEL: i64 = -1_009_876_543_210;

    const HEART: &str = "\u{2764}";
    const HEART_WITH_SELECTOR: &str = "\u{2764}\u{FE0F}";

    fn user(id: i64) -> serde_json::Value {
        json!({"@type": "messageSenderUser", "user_id": id})
    }

    fn emoji(e: &str) -> serde_json::Value {
        json!({"@type": "reactionTypeEmoji", "emoji": e})
    }

    fn custom(id: i64) -> serde_json::Value {
        json!({"@type": "reactionTypeCustomEmoji", "custom_emoji_id": id.to_string()})
    }

    /// One `messageReaction` as TDLib sends it. `used` is the sender the
    /// account reacted as, which TDLib gives only when it names that sender
    /// among the recent ones.
    fn reaction(
        kind: serde_json::Value,
        total: i32,
        chosen: bool,
        used: Option<serde_json::Value>,
        recent: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let mut r = json!({
            "@type": "messageReaction",
            "type": kind,
            "total_count": total,
            "is_chosen": chosen,
            "recent_sender_ids": recent,
        });
        if let Some(used) = used {
            r["used_sender_id"] = used;
        }
        r
    }

    /// A `messageInteractionInfo` holding these reactions, read from JSON as
    /// TDLib would send it.
    fn info(reactions: Vec<serde_json::Value>) -> MessageInteractionInfo {
        serde_json::from_value(json!({
            "@type": "messageInteractionInfo",
            "view_count": 0,
            "forward_count": 0,
            "reactions": {
                "@type": "messageReactions",
                "reactions": reactions,
                "are_tags": false,
                "paid_reactors": [],
                "can_get_added_reactions": true,
            },
        }))
        .expect("fixture is a valid messageInteractionInfo")
    }

    fn user_sender(id: i64) -> MessageSender {
        serde_json::from_value(user(id)).unwrap()
    }

    fn listed(reactions: Vec<serde_json::Value>) -> Vec<ListedReaction> {
        listed_reactions(Some(&info(reactions)), &CustomEmoji::new())
    }

    #[test]
    fn a_message_without_interaction_info_lists_nothing() {
        assert!(listed_reactions(None, &CustomEmoji::new()).is_empty());
        // Views and forwards alone: a channel post nobody reacted to.
        let viewed: MessageInteractionInfo = serde_json::from_value(json!({
            "@type": "messageInteractionInfo",
            "view_count": 120,
            "forward_count": 3,
        }))
        .unwrap();
        assert!(listed_reactions(Some(&viewed), &CustomEmoji::new()).is_empty());
    }

    #[test]
    fn each_emoji_is_listed_with_its_count_and_senders_in_tdlibs_order() {
        let got = listed(vec![
            reaction(emoji("👍"), 2, false, None, vec![user(GIULIA), user(PAOLO)]),
            reaction(emoji(HEART), 1, false, None, vec![user(MARCO)]),
        ]);
        assert_eq!(
            got,
            vec![
                ListedReaction {
                    emoji: "👍".to_string(),
                    count: 2,
                    chosen: false,
                    senders: vec![user_sender(GIULIA), user_sender(PAOLO)],
                    chosen_types: vec![],
                },
                ListedReaction {
                    emoji: HEART.to_string(),
                    count: 1,
                    chosen: false,
                    senders: vec![user_sender(MARCO)],
                    chosen_types: vec![],
                },
            ]
        );
    }

    #[test]
    fn the_account_is_chosen_and_never_among_the_senders() {
        // A group: TDLib names the account first, as the sender it reacted as,
        // and two other people. The count is everyone.
        let got = listed(vec![reaction(
            emoji("👍"),
            3,
            true,
            Some(user(ME)),
            vec![user(ME), user(GIULIA), user(PAOLO)],
        )]);
        assert_eq!(got.len(), 1);
        assert!(got[0].chosen);
        assert_eq!(got[0].count, 3);
        assert_eq!(
            got[0].senders,
            vec![user_sender(GIULIA), user_sender(PAOLO)]
        );
        // So the number nobody is named for is count - chosen - senders: 0.
        assert_eq!(got[0].count - 1 - got[0].senders.len() as i32, 0);
    }

    #[test]
    fn a_one_to_one_chat_names_the_other_person_alone() {
        // In a private chat TDLib builds the senders itself: the account when
        // it reacted, then the other person when the count leaves room.
        let both = listed(vec![reaction(
            emoji(HEART),
            2,
            true,
            Some(user(ME)),
            vec![user(ME), user(GIULIA)],
        )]);
        assert_eq!(both[0].senders, vec![user_sender(GIULIA)]);

        let mine_alone = listed(vec![reaction(
            emoji(HEART),
            1,
            true,
            Some(user(ME)),
            vec![user(ME)],
        )]);
        assert!(mine_alone[0].chosen);
        assert!(mine_alone[0].senders.is_empty());
    }

    #[test]
    fn a_reaction_of_the_account_tdlib_does_not_name_keeps_every_named_sender() {
        // A busy group: the account reacted, but it is not among the three
        // recent senders, so TDLib gives no used_sender_id. Nobody is dropped.
        let got = listed(vec![reaction(
            emoji("🔥"),
            40,
            true,
            None,
            vec![user(GIULIA), user(PAOLO), user(MARCO)],
        )]);
        assert!(got[0].chosen);
        assert_eq!(got[0].senders.len(), 3);
    }

    #[test]
    fn a_large_group_gives_a_count_alone() {
        let got = listed(vec![reaction(emoji("🔥"), 40, false, None, vec![])]);
        assert_eq!(got[0].count, 40);
        assert!(got[0].senders.is_empty());
    }

    #[test]
    fn a_reaction_made_as_a_chat_keeps_the_chat_as_its_sender() {
        let got = listed(vec![reaction(
            emoji("👍"),
            1,
            false,
            None,
            vec![json!({"@type": "messageSenderChat", "chat_id": CHANNEL})],
        )]);
        assert_eq!(
            got[0].senders,
            vec![
                serde_json::from_value::<MessageSender>(
                    json!({"@type": "messageSenderChat", "chat_id": CHANNEL})
                )
                .unwrap()
            ]
        );
    }

    #[test]
    fn a_paid_reaction_is_left_out() {
        let got = listed(vec![
            reaction(
                json!({"@type": "reactionTypePaid"}),
                250,
                false,
                None,
                vec![],
            ),
            reaction(emoji("👍"), 1, false, None, vec![user(GIULIA)]),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].emoji, "👍");
    }

    #[test]
    fn a_custom_emoji_is_listed_as_the_emoji_it_stands_for() {
        let names = CustomEmoji::from([(7_001, "🎉".to_string())]);
        let got = listed_reactions(
            Some(&info(vec![reaction(
                custom(7_001),
                4,
                false,
                None,
                vec![user(GIULIA)],
            )])),
            &names,
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].emoji, "🎉");
        assert_eq!(got[0].count, 4);
    }

    #[test]
    fn a_custom_emoji_tdlib_cannot_name_is_left_out() {
        let got = listed(vec![
            reaction(custom(7_002), 4, false, None, vec![user(GIULIA)]),
            reaction(emoji("👍"), 1, false, None, vec![user(PAOLO)]),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].emoji, "👍");
    }

    #[test]
    fn a_custom_emoji_joins_the_standard_emoji_it_stands_for() {
        // A custom heart whose sticker names "❤️" (with the selector) and
        // Telegram's own bare heart are one emoji: one entry, in Telegram's
        // spelling, with both counts, both senders and both of the account's
        // reactions to take back.
        let names = CustomEmoji::from([(7_003, HEART_WITH_SELECTOR.to_string())]);
        let got = listed_reactions(
            Some(&info(vec![
                reaction(
                    custom(7_003),
                    2,
                    true,
                    Some(user(ME)),
                    vec![user(ME), user(GIULIA)],
                ),
                reaction(
                    emoji(HEART),
                    3,
                    false,
                    None,
                    vec![user(GIULIA), user(PAOLO)],
                ),
            ])),
            &names,
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].emoji, HEART);
        assert_eq!(got[0].count, 5);
        assert!(got[0].chosen);
        assert_eq!(
            got[0].senders,
            vec![user_sender(GIULIA), user_sender(PAOLO)]
        );
        assert_eq!(
            got[0].chosen_types,
            vec![serde_json::from_value::<ReactionType>(custom(7_003)).unwrap()]
        );
    }

    #[test]
    fn the_accounts_own_reaction_is_kept_as_the_type_to_take_back() {
        let got = listed(vec![
            reaction(
                emoji("👍"),
                2,
                true,
                Some(user(ME)),
                vec![user(ME), user(GIULIA)],
            ),
            reaction(emoji(HEART), 1, false, None, vec![user(PAOLO)]),
        ]);
        assert_eq!(
            got[0].chosen_types,
            vec![serde_json::from_value::<ReactionType>(emoji("👍")).unwrap()]
        );
        assert!(got[1].chosen_types.is_empty());
    }

    #[test]
    fn chosen_types_are_the_accounts_reactions_listed_as_that_emoji() {
        let names = CustomEmoji::from([(7_003, HEART_WITH_SELECTOR.to_string())]);
        let i = info(vec![
            reaction(
                emoji("👍"),
                2,
                true,
                Some(user(ME)),
                vec![user(ME), user(GIULIA)],
            ),
            reaction(custom(7_003), 1, true, Some(user(ME)), vec![user(ME)]),
            reaction(emoji("🔥"), 1, false, None, vec![user(PAOLO)]),
        ]);
        let reaction_type = |v| serde_json::from_value::<ReactionType>(v).unwrap();

        assert_eq!(
            chosen_types(Some(&i), &names, "👍"),
            vec![reaction_type(emoji("👍"))]
        );
        // The custom heart is listed as a heart, and taken back by either
        // spelling of it.
        for heart in [HEART, HEART_WITH_SELECTOR] {
            assert_eq!(
                chosen_types(Some(&i), &names, heart),
                vec![reaction_type(custom(7_003))],
                "{heart:?}"
            );
        }
        // Someone else's reaction is not the account's to take back.
        assert!(chosen_types(Some(&i), &names, "🔥").is_empty());
        assert!(chosen_types(None, &names, "👍").is_empty());
    }

    #[test]
    fn custom_emoji_ids_are_collected_once_each() {
        let i = info(vec![
            reaction(custom(7_001), 1, false, None, vec![]),
            reaction(emoji("👍"), 1, false, None, vec![]),
            reaction(custom(7_002), 1, false, None, vec![]),
            reaction(custom(7_001), 1, false, None, vec![]),
        ]);
        assert_eq!(custom_emoji_ids(Some(&i)), vec![7_001, 7_002]);
        assert!(custom_emoji_ids(None).is_empty());
    }

    /// A custom emoji sticker as `getCustomEmojiStickers` answers it.
    fn sticker(custom_emoji_id: i64, stands_for: &str) -> Sticker {
        serde_json::from_value(json!({
            "@type": "sticker",
            "id": custom_emoji_id.to_string(),
            "set_id": "9001",
            "width": 100,
            "height": 100,
            "emoji": stands_for,
            "format": {"@type": "stickerFormatWebp"},
            "full_type": {
                "@type": "stickerFullTypeCustomEmoji",
                "custom_emoji_id": custom_emoji_id.to_string(),
                "needs_repainting": false,
            },
            "sticker": {
                "@type": "file",
                "id": 1,
                "size": 0,
                "expected_size": 0,
                "local": {
                    "@type": "localFile",
                    "path": "",
                    "can_be_downloaded": true,
                    "can_be_deleted": false,
                    "is_downloading_active": false,
                    "is_downloading_completed": false,
                    "download_offset": 0,
                    "downloaded_prefix_size": 0,
                    "downloaded_size": 0,
                },
                "remote": {
                    "@type": "remoteFile",
                    "id": "",
                    "unique_id": "",
                    "is_uploading_active": false,
                    "is_uploading_completed": true,
                    "uploaded_size": 0,
                },
            },
        }))
        .expect("fixture is a valid sticker")
    }

    #[test]
    fn a_custom_emoji_sticker_names_the_emoji_it_stands_for() {
        let names = custom_emoji_names(&[sticker(7_001, "🎉"), sticker(7_002, "")]);
        assert_eq!(names, CustomEmoji::from([(7_001, "🎉".to_string())]));
    }

    #[test]
    fn emoji_key_drops_the_variation_selector_and_nothing_else() {
        assert_eq!(emoji_key(HEART_WITH_SELECTOR), HEART);
        assert_eq!(emoji_key(HEART), HEART);
        // A joined sequence keeps its joiner: heart on fire.
        assert_eq!(
            emoji_key("\u{2764}\u{FE0F}\u{200D}\u{1F525}"),
            "\u{2764}\u{200D}\u{1F525}"
        );
        // A skin tone is part of the emoji, not a spelling of it.
        assert_eq!(emoji_key("👍🏽"), "👍🏽");
        assert_ne!(emoji_key("👍🏽"), emoji_key("👍"));
        assert_eq!(emoji_key("🇮🇹"), "🇮🇹");
    }

    fn offered() -> Vec<String> {
        [
            "👍",
            HEART,
            "🔥",
            "\u{2764}\u{200D}\u{1F525}",
            "🤷\u{200D}\u{2642}",
        ]
        .iter()
        .map(|e| e.to_string())
        .collect()
    }

    #[test]
    fn telegram_form_keeps_an_emoji_telegram_offers_as_given() {
        assert_eq!(telegram_form("👍", &offered()), Some("👍".to_string()));
        assert_eq!(telegram_form(HEART, &offered()), Some(HEART.to_string()));
    }

    #[test]
    fn telegram_form_answers_telegrams_spelling_of_a_keyboard_emoji() {
        // The heart a keyboard types carries the selector; Telegram's does not.
        assert_eq!(
            telegram_form(HEART_WITH_SELECTOR, &offered()),
            Some(HEART.to_string())
        );
        // Likewise inside a joined sequence.
        assert_eq!(
            telegram_form("\u{2764}\u{FE0F}\u{200D}\u{1F525}", &offered()),
            Some("\u{2764}\u{200D}\u{1F525}".to_string())
        );
        assert_eq!(
            telegram_form("🤷\u{200D}\u{2642}\u{FE0F}", &offered()),
            Some("🤷\u{200D}\u{2642}".to_string())
        );
    }

    #[test]
    fn telegram_form_goes_the_other_way_too() {
        // Should Telegram ever offer an emoji with the selector, a bare one
        // asked for is that emoji as well.
        let with_selector = vec![HEART_WITH_SELECTOR.to_string()];
        assert_eq!(
            telegram_form(HEART, &with_selector),
            Some(HEART_WITH_SELECTOR.to_string())
        );
    }

    #[test]
    fn telegram_form_has_no_answer_for_an_emoji_telegram_does_not_offer() {
        assert_eq!(telegram_form("🦄", &offered()), None);
        // A skin tone is another emoji, not another spelling.
        assert_eq!(telegram_form("👍🏽", &offered()), None);
        assert_eq!(telegram_form("", &offered()), None);
    }
}
