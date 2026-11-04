//! A message's formatting as `marks` (AGENTS.md, *A message's `text`*): TDLib's
//! entities over a `FormattedText`, turned into the five kinds a reader draws
//! and into UTF-8 byte offsets of the `text` tg writes.

use crate::output::{MarkKind, TextMark};
use tdlib_rs::enums::TextEntityType;
use tdlib_rs::types::FormattedText;

/// The most marks one message carries.
const MOST_MARKS: usize = 500;
/// The longest address a link mark carries, in bytes.
const LONGEST_URL: usize = 2048;

/// The marks of `text`'s entities, for the words placed `prefix_bytes` bytes
/// into the `text` tg writes (a caption sits behind its bracket's opening).
///
/// TDLib counts UTF-16 code units; a mark counts UTF-8 bytes. Kinds: bold,
/// italic, strikethrough, a text link with its URL, and code (`Code`, `Pre`,
/// `PreCode`); every other entity shows plain. Ordered by `at`, the longer
/// first, at most 500; a link whose address is not http, https or mailto, or
/// is over 2,048 bytes, makes no mark, nor does one that overlaps a link kept
/// before it, nor an entity that is empty, out of bounds or starts or ends in
/// the middle of a character.
pub fn marks(text: &FormattedText, prefix_bytes: usize) -> Vec<TextMark> {
    if text.entities.is_empty() {
        return Vec::new();
    }
    // `bytes_at[u]` is the byte offset of UTF-16 unit `u`, `None` inside a
    // surrogate pair.
    let mut bytes_at: Vec<Option<usize>> = Vec::with_capacity(text.text.len() + 1);
    for (byte, c) in text.text.char_indices() {
        bytes_at.push(Some(byte));
        if c.len_utf16() == 2 {
            bytes_at.push(None);
        }
    }
    bytes_at.push(Some(text.text.len()));

    let mut found: Vec<TextMark> = Vec::new();
    for entity in &text.entities {
        let (kind, url) = match &entity.r#type {
            TextEntityType::Bold => (MarkKind::Bold, None),
            TextEntityType::Italic => (MarkKind::Italic, None),
            TextEntityType::Strikethrough => (MarkKind::Strike, None),
            TextEntityType::Code | TextEntityType::Pre | TextEntityType::PreCode(_) => {
                (MarkKind::Code, None)
            }
            TextEntityType::TextUrl(t) if link_allowed(&t.url) => {
                (MarkKind::Link, Some(t.url.clone()))
            }
            _ => continue,
        };
        let (Ok(start), Ok(length)) = (
            usize::try_from(entity.offset),
            usize::try_from(entity.length),
        ) else {
            continue;
        };
        let Some(end) = start.checked_add(length) else {
            continue;
        };
        let (Some(Some(from)), Some(Some(to))) = (bytes_at.get(start), bytes_at.get(end)) else {
            continue;
        };
        if to <= from {
            continue;
        }
        found.push(TextMark {
            kind,
            at: prefix_bytes + from,
            len: to - from,
            url,
        });
    }

    found.sort_by(|a, b| a.at.cmp(&b.at).then(b.len.cmp(&a.len)));
    let mut kept: Vec<TextMark> = Vec::with_capacity(found.len());
    for mark in found {
        if kept.contains(&mark) {
            continue;
        }
        if mark.kind == MarkKind::Link
            && kept.iter().any(|k| {
                k.kind == MarkKind::Link && mark.at < k.at + k.len && k.at < mark.at + mark.len
            })
        {
            continue;
        }
        kept.push(mark);
    }
    kept.truncate(MOST_MARKS);
    kept
}

fn link_allowed(url: &str) -> bool {
    if url.len() > LONGEST_URL {
        return false;
    }
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    ["http", "https", "mailto"]
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tdlib_rs::types::TextEntity;

    fn entity(offset: i32, length: i32, r#type: TextEntityType) -> TextEntity {
        TextEntity {
            offset,
            length,
            r#type,
        }
    }

    fn text_url(url: &str) -> TextEntityType {
        TextEntityType::TextUrl(tdlib_rs::types::TextEntityTypeTextUrl {
            url: url.to_string(),
        })
    }

    fn formatted(text: &str, entities: Vec<TextEntity>) -> FormattedText {
        FormattedText {
            text: text.to_string(),
            entities,
        }
    }

    fn json(marks: &[TextMark]) -> String {
        serde_json::to_string(marks).unwrap()
    }

    #[test]
    fn marks_of_a_text() {
        let t = formatted("See you Saturday", vec![entity(8, 8, TextEntityType::Bold)]);
        assert_eq!(json(&marks(&t, 0)), r#"[{"k":"bold","at":8,"len":8}]"#);
    }

    #[test]
    fn marks_of_a_caption_shift_by_the_bracket() {
        let t = formatted(
            "Sunset over the bay",
            vec![entity(0, 6, TextEntityType::Italic)],
        );
        let m = marks(&t, "[Photo: ".len());
        assert_eq!(json(&m), r#"[{"k":"italic","at":8,"len":6}]"#);
    }

    #[test]
    fn marks_count_bytes_not_utf16() {
        // 🙂 is 2 UTF-16 units and 4 bytes, è 1 unit and 2 bytes.
        let t = formatted(
            "🙂 è docs here",
            vec![entity(5, 4, text_url("https://example.com/docs"))],
        );
        let m = marks(&t, 0);
        assert_eq!(&t.text[m[0].at..m[0].at + m[0].len], "docs");
        assert_eq!((m[0].at, m[0].len), (8, 4));
        assert_eq!(m[0].url.as_deref(), Some("https://example.com/docs"));
        assert_eq!(
            json(&m),
            r#"[{"k":"link","at":8,"len":4,"url":"https://example.com/docs"}]"#
        );
    }

    #[test]
    fn kinds_that_make_no_mark() {
        use tdlib_rs::types::TextEntityTypeMentionName;
        let kinds = vec![
            TextEntityType::Url,
            TextEntityType::EmailAddress,
            TextEntityType::Mention,
            TextEntityType::MentionName(TextEntityTypeMentionName { user_id: 1 }),
            TextEntityType::Hashtag,
            TextEntityType::Cashtag,
            TextEntityType::BotCommand,
            TextEntityType::PhoneNumber,
            TextEntityType::Underline,
            TextEntityType::Spoiler,
            TextEntityType::BlockQuote,
            TextEntityType::ExpandableBlockQuote,
            TextEntityType::MediaTimestamp(tdlib_rs::types::TextEntityTypeMediaTimestamp {
                media_timestamp: 5,
            }),
            TextEntityType::CustomEmoji(tdlib_rs::types::TextEntityTypeCustomEmoji {
                custom_emoji_id: 7,
            }),
        ];
        for kind in kinds {
            let t = formatted("some words here", vec![entity(0, 4, kind.clone())]);
            assert!(marks(&t, 0).is_empty(), "{kind:?} should make no mark");
        }
    }

    #[test]
    fn pre_and_code_are_code() {
        let t = formatted(
            "run ls now",
            vec![
                entity(4, 2, TextEntityType::Code),
                entity(0, 3, TextEntityType::Pre),
                entity(
                    7,
                    3,
                    TextEntityType::PreCode(tdlib_rs::types::TextEntityTypePreCode {
                        language: "sh".to_string(),
                    }),
                ),
            ],
        );
        assert_eq!(
            json(&marks(&t, 0)),
            r#"[{"k":"code","at":0,"len":3},{"k":"code","at":4,"len":2},{"k":"code","at":7,"len":3}]"#
        );
    }

    #[test]
    fn nested_entities_are_both_kept() {
        let t = formatted(
            "all of it bold and more",
            vec![
                entity(7, 2, TextEntityType::Italic),
                entity(0, 11, TextEntityType::Bold),
            ],
        );
        assert_eq!(
            json(&marks(&t, 0)),
            r#"[{"k":"bold","at":0,"len":11},{"k":"italic","at":7,"len":2}]"#
        );
    }

    #[test]
    fn the_longer_comes_first_at_one_offset() {
        let t = formatted(
            "abcdef",
            vec![
                entity(0, 2, TextEntityType::Italic),
                entity(0, 6, TextEntityType::Bold),
            ],
        );
        let m = marks(&t, 0);
        assert_eq!((m[0].kind, m[0].len), (MarkKind::Bold, 6));
        assert_eq!((m[1].kind, m[1].len), (MarkKind::Italic, 2));
    }

    #[test]
    fn a_disallowed_url_makes_no_link() {
        for url in ["tg://user?id=1", "javascript:alert(1)", "no scheme", ""] {
            let t = formatted("click here", vec![entity(0, 5, text_url(url))]);
            assert!(marks(&t, 0).is_empty(), "{url:?}");
        }
        for url in [
            "HTTP://example.com",
            "Https://example.com",
            "MAILTO:a@example.com",
        ] {
            let t = formatted("click here", vec![entity(0, 5, text_url(url))]);
            assert_eq!(marks(&t, 0).len(), 1, "{url:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(2048));
        let t = formatted("click here", vec![entity(0, 5, text_url(&long))]);
        assert!(marks(&t, 0).is_empty());
    }

    #[test]
    fn an_exact_duplicate_is_kept_once() {
        let t = formatted(
            "abcdef",
            vec![
                entity(1, 3, TextEntityType::Bold),
                entity(1, 3, TextEntityType::Italic),
                entity(1, 3, TextEntityType::Bold),
            ],
        );
        assert_eq!(marks(&t, 0).len(), 2);
    }

    #[test]
    fn two_links_never_overlap() {
        let t = formatted(
            "abcdefgh",
            vec![
                entity(0, 5, text_url("https://example.com/a")),
                entity(3, 4, text_url("https://example.com/b")),
                entity(5, 3, text_url("https://example.com/c")),
            ],
        );
        let m = marks(&t, 0);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].at, m[0].len), (0, 5));
        assert_eq!((m[1].at, m[1].len), (5, 3));
    }

    #[test]
    fn bad_ranges_make_no_mark() {
        let t = formatted(
            "a🙂b",
            vec![
                entity(0, 0, TextEntityType::Bold),
                entity(0, 99, TextEntityType::Bold),
                entity(-1, 2, TextEntityType::Bold),
                entity(2, 1, TextEntityType::Bold), // inside the surrogate pair
                entity(0, 2, TextEntityType::Bold), // ends inside it
            ],
        );
        assert!(marks(&t, 0).is_empty());
    }

    #[test]
    fn at_most_five_hundred() {
        let text = "x".repeat(600);
        let entities = (0..600)
            .map(|i| entity(i, 1, TextEntityType::Bold))
            .collect();
        let m = marks(&formatted(&text, entities), 0);
        assert_eq!(m.len(), 500);
        assert_eq!(m[499].at, 499);
    }

    #[test]
    fn an_empty_caption_has_no_marks() {
        assert!(marks(&formatted("", vec![]), 8).is_empty());
        assert!(marks(&formatted("", vec![entity(0, 4, TextEntityType::Bold)]), 8).is_empty());
    }
}
