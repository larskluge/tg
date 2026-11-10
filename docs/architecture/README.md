# Architecture

How `tg` works as built, one file per area. The rules every change keeps, the layout and the
everyday commands are in [AGENTS.md](../../AGENTS.md); the contracts other programs read (the CLI's
output, the serve protocol, a message's `text`, reactions) are in the [README](../../README.md) and
`docs/specs/2026-05-22-serve-design.md`.

Each section says where its TDLib facts come from (TDLib's source at a named commit, `td_api.tl`,
or a measurement) and at which version; the reaction poll's are to be checked again when tdlib-rs
moves to another TDLib.

- [history.md](history.md): `getChatHistory`'s quirk and offsets, walking forward
  (`sync --oldest-first`), `getChatMessageByDate`'s direction, and a deleted private chat
  (`sync --report-deleted`).
- [chats.md](chats.md): a channel is a supergroup with `is_channel`; deleting a private chat
  (`delete_chat`).
- [messages.md](messages.md): a message's `text` and its caption, its `marks`, and `reply_to`.
- [sending.md](sending.md): TDLib's HTML and MarkdownV2 parse modes, an explicit `null`,
  attachments, a partial album, one file, and a voice note.
- [serve.md](serve.md): subscriptions (`subscribe`, `tg stream`) and serve request strictness.
- [reactions.md](reactions.md): listing, ringing and sending reactions, and the reaction poll.

The notes behind the tests that talk to a real TDLib are in
[docs/build-and-test.md](../build-and-test.md).
