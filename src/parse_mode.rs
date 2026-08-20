//! The message-formatting mode carried on `tg send`. This is a protocol value:
//! the same two literals travel on the serve socket (`args.parse_mode`), on the
//! CLI (`--parse-mode`), and in the Telegram Bot API payload. Absent means the
//! body is sent verbatim as plain text.

use crate::error::{Result, TgError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    Html,
    MarkdownV2,
}

impl ParseMode {
    /// Strict, case-sensitive. The accepted set is exactly the two literals the
    /// wire contract names; anything else is a caller bug and is refused rather
    /// than downgraded to plain text — a body a human approved as formatted must
    /// never go out looking like something else without a signal.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "HTML" => Ok(Self::Html),
            "MarkdownV2" => Ok(Self::MarkdownV2),
            other => Err(TgError::Other(format!(
                "invalid parse_mode '{other}'. Expected `HTML` or `MarkdownV2`"
            ))),
        }
    }

    /// The canonical wire literal. Also exactly what the Bot API expects, so
    /// the socket contract and the HTTP payload cannot drift.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Html => "HTML",
            Self::MarkdownV2 => "MarkdownV2",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_html_accepts_exact() {
        assert_eq!(ParseMode::parse("HTML").unwrap(), ParseMode::Html);
    }

    #[test]
    fn parse_markdown_v2_accepts_exact() {
        assert_eq!(
            ParseMode::parse("MarkdownV2").unwrap(),
            ParseMode::MarkdownV2
        );
    }

    #[test]
    fn parse_rejects_wrong_case() {
        // Case-insensitivity would be a contract widening that can never be
        // walked back, so the near-misses must stay rejected.
        for value in ["html", "Html", "markdownv2", "MARKDOWNV2", "markdownV2"] {
            assert!(
                ParseMode::parse(value).is_err(),
                "{value} must not be accepted"
            );
        }
    }

    #[test]
    fn parse_rejects_unknown_value() {
        // "Markdown" specifically: TDLib parser versions 0/1 are the legacy,
        // laxer mode and must not be reachable through this contract.
        for value in ["Markdown", "md", "plain", "markdown-v2", "text", "None"] {
            let err = ParseMode::parse(value).unwrap_err().to_string();
            assert!(
                err.contains(value),
                "error should name the bad value: {err}"
            );
            assert!(err.contains("HTML"), "error should name HTML: {err}");
            assert!(
                err.contains("MarkdownV2"),
                "error should name MarkdownV2: {err}"
            );
        }
    }

    #[test]
    fn parse_rejects_empty_and_whitespace() {
        // An empty string is a caller bug (an uninterpolated variable), never a
        // synonym for absent — treating it as "send plain" would deliver an
        // unformatted body silently. No trimming either.
        for value in ["", " ", "  ", " HTML", "HTML ", "\tMarkdownV2"] {
            assert!(
                ParseMode::parse(value).is_err(),
                "{value:?} must not be accepted"
            );
        }
    }

    #[test]
    fn parse_error_text_is_exact() {
        // Wire contract: a caller may match on this string, so a silent reword
        // must break a test.
        assert_eq!(
            ParseMode::parse("xyz").unwrap_err().to_string(),
            "invalid parse_mode 'xyz'. Expected `HTML` or `MarkdownV2`"
        );
    }

    #[test]
    fn as_str_matches_wire_literals() {
        assert_eq!(ParseMode::Html.as_str(), "HTML");
        assert_eq!(ParseMode::MarkdownV2.as_str(), "MarkdownV2");
    }

    #[test]
    fn as_str_round_trips_through_parse() {
        // `as_str` feeds the Bot API payload while `parse` guards the socket and
        // CLI; this is what stops the two wires drifting apart.
        for mode in [ParseMode::Html, ParseMode::MarkdownV2] {
            assert_eq!(ParseMode::parse(mode.as_str()).unwrap(), mode);
        }
    }
}
