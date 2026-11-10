# Messages

What a listed message carries: its `text`, its `marks` and the message it replies to. The rules
every change keeps are in [AGENTS.md](../../AGENTS.md); this file is how it works as built.

**A message's `text` is a body other programs store (its caption where it has one, since 0.14.0
for animation, audio and voice, 0.16.0 for paid media):** Mycelium stores `text` as the message's body unparsed, and
Tavola and every agent read that body. The shapes are in `README.md` → *A message's `text`, and
its caption*.

- **`extract_message_data` is the one writer of a message's text.** Every listing reaches it
  through `message_info` (history pages via `message_infos`, `getMessage` for `message`), and a
  chat row's `last_message` through `extract_message_text`. `subscribe` frames carry no text, and
  an edit is read again through the same path, so an edited caption changes `text`. Change a
  shape there and nowhere else.
- **A caption takes the measures' place in the bracket**, for every content that has one:
  `[Photo: <caption>]`, `[Video: …]`, `[Document: …]`, `[Animation: …]`, `[Audio: …]`,
  `[Voice: …]`, `[Paid media: …]`. `non_empty` decides what counts as a caption, so whitespace
  alone is none. Without one the text is the measures, byte for byte as before 0.14.0; paid media,
  whose measure is the price (`[Paid media: 25 stars]`), was `Paid media (25 stars)`, unbracketed,
  before 0.16.0 (tavola-i4cx; Mycelium held no paid media on 2026-10-18). `content` (with its
  `caption` and measures), `content_type` and the files do not change with it. A video note has no
  caption in TDLib.
- **`marks` sits beside `text`, written by the same one writer (since 0.19.0).**
  `extract_message_data` turns the `FormattedText`'s entities (a text's, and every caption's: photo,
  video, document, audio, voice, animation, paid media) into `marks` (`src/marks.rs`),
  `message_info` copies them to `MessageInfo.marks`, and an edit is read again through the same
  path, so it brings its marks. Each is `{"k","at","len"}` (and `"url"` on a link); `at` and `len`
  count **UTF-8 bytes of that `text`**, where TDLib counts UTF-16 units, so a caption's marks are
  shifted by its bracket (`[Photo: `, `[Paid media: `, …). Only five kinds: `bold`, `italic`,
  `strike`, `code` (`Code`, `Pre`, `PreCode`) and `link` (`TextUrl`, its URL http, https or mailto,
  at most 2,048 bytes). Every other entity (`Url`, `EmailAddress`, `Mention`, `Hashtag`,
  `Underline`, `Spoiler`, `BlockQuote`, `CustomEmoji`, …) shows plain. Ordered by `at`, the longer
  first; at most 500; two links never overlap; an empty or out-of-range entity, or one that starts
  or ends inside a character, makes none. A whitespace-only caption is no caption and has none, and
  a poll's question carries only custom emoji, so a poll has none. The key is left out where there
  are no marks, and `text` does not change. The record has no version field; a reader that does not
  know `marks` passes it by.
- **A new shape does not rewrite what Mycelium already holds.** It keys a Telegram message on
  `tg:<chat>:<message>:<edit_date>` and records a message again only when that changes, so a
  message ingested before the change keeps its old body until it is edited.

**TDLib `message.reply_to` names a chat, not just a message (since 0.8.0):**
`messageReplyToMessage { chat_id, message_id, … }` — `chat_id` is the chat of the *replied*
message. For an ordinary reply it equals the message's own `chat_id` (measured against 1.8.61:
33 replies across 3 groups, zero mismatches); it differs for a cross-chat reply (the Replies chat,
a quote from elsewhere), and both ids are `0` when that chat is unknown. A message id is only
meaningful with its chat, and Mycelium keys Telegram messages on `(chat_id, message_id)`, so
`reply_in_chat` is the ONE reader of `reply_to`: it yields the id only for a same-chat message
reply and `None` for cross-chat, unknown-chat and story replies. Listing (`reply_to_message_id`
on every `MessageInfo`) and send confirmation both go through it; never read
`r.message_id` bare. A cross-chat reply is dropped rather than exported with its chat, because
the one consumer reads the bare id.
