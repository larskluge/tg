# Sending

`tg send` and the serve `send` request: parse modes, attachments, albums and voice notes. The rules
every change keeps are in [AGENTS.md](../../AGENTS.md); this file is how it works as built.

**TDLib HTML parse mode is not HTML:** `textParseModeHTML` accepts only Telegram's tag
whitelist — `b`/`strong`, `i`/`em`, `u`/`ins`, `s`/`strike`/`del`, `a href`, `code`, `pre`
(+ `code class="language-x"`), `blockquote` (optionally `expandable`), `tg-spoiler`,
`tg-emoji`. There are no headings, lists, `hr`, `p` or `br`. TDLib **errors** on any tag
outside the set (it does not strip it), so `<h1>x</h1>` fails the send rather than degrading.
Tag names are case-insensitive. Only `&`, `<`, `>` need escaping; escape all three
unconditionally, never selectively. Related facts, all measured against TDLib 1.8.61:

- `parse_text_entities` returns `enums::FormattedText`, so it needs `unwrap_formatted_text()`
  before it can go into `InputMessageText` (see **TDLib types** in [AGENTS.md](../../AGENTS.md)). It also takes an
  owned `String`.
- TDLib does **not** auto-detect bold/italic/code server-side — its entity auto-detection
  covers only link-ish entities. That is why `entities: vec![]` delivered literal asterisks
  for years, and why a parse mode has to be requested explicitly.
- Entity offsets are UTF-16 and TDLib computes them. Never hand-roll them: always call
  `parse_text_entities`. Verified exact across emoji, umlauts, combining marks, ZWJ sequences
  and regional indicators (🚀 = 2 units, 👩‍💻 = 5, 🇩🇪 = 4 — that last one is 2 *code points*,
  and reading the code-point count as the unit count is a two-unit error exactly where flags
  appear), and the tdlib-rs hop is lossless because `types::FormattedText` and
  `enums::TextEntityType` both round-trip through serde. Pinned end to end by
  `entity_offsets_are_utf16_and_tg_never_touches_them`.
- `MarkdownV2` is parser `version: 2`. Versions 0 and 1 are the legacy, laxer "Markdown"
  mode: picking one by mistake parses the body under the wrong rules with no error anywhere
  (pinned by `tdlib_parse_mode_markdown_is_version_2`).
- `parseTextEntities` is a TDLib **static request** — it answers on a client that has never
  called `setTdlibParameters`, needs no authorization, makes no network call, and costs
  ~17 µs. Calling it before `create_private_chat` therefore leaves zero residue when the
  markup is bad: no message, no draft, no opened chat.
- Neither mode is free of silent corruption; `HTML` is safer, not safe. Under `HTML` a
  matched pair of whitelisted tags anywhere in the body becomes formatting
  (`wrap it in <b>...</b> tags` → `wrap it in ... tags`, `ok:true`), and entity decoding runs
  exactly once, so pre-escaped prose is un-escaped. Under `MarkdownV2` a reserved character
  alone hard-errors, but a matching *pair* is eaten silently — `` _ * ~ ` __ || […] `` — as is
  a line-leading `>` (blockquote) and a backslash anywhere, even singly. `\` is not in the
  documented eighteen at all. `path /usr/local/bin/x_y_z` → `path /usr/local/bin/xyz`,
  `C:\Data\2026` → `C:Data2026`. Prefer `HTML`, and escape unconditionally.
- HTML entity decoding is narrower than the docs suggest for *named* entities and wider for
  numeric ones: only `lt`/`gt`/`amp`/`quot` decode by name (`&nbsp;`, `&apos;`, `&copy;` stay
  literal), but **every** numeric character reference decodes (`&#8364;` → `€`,
  `&#x41;` → `A`). `&#xD800;` is a hard error ("unmatched surrogate code units"). This is why
  `&` must be escaped unconditionally rather than only before a known entity name.
- Errors are TDLib's own text, prefixed `parse_mode <MODE>: `. Only the unsupported-tag and
  unterminated-entity classes name a byte offset; `Character 'x' is reserved` does not. Do
  not write a caller-side repair loop that depends on finding one.

An explicit `"parse_mode": null` is accepted and means plain text, exactly like an absent
key. `"files": null` is accepted the same way and for the same reason. That asymmetry with `""`
(refused) is deliberate: `null` is what an unset optional
serialises to in Go (`map[string]any` with a missing key, or a `*string` without
`omitempty`), in Python (`json.dumps`) and in `tg`'s own CLI proxy, so refusing it would
turn every plain-text send from those callers into a hard error while closing nothing — a
caller that omits the key entirely still gets a plain send, and "absent means plain" is the
contract. Pinned by `send_request_accepts_explicit_null_parse_mode`.

**Attachments (`args.files`, since 0.5.0):** 1-10 local files sent as ONE message, `args.message`
being the caption. Validated in `send::handle` BEFORE the recipient ladder — same ordering and
reason as `parse_mode`, plus one of its own: TDLib uploads a file *after* the send returns, so a
path it cannot read would otherwise surface as a failed send on a message the caller was already
told about. Three facts worth not re-deriving:

- **No TDLib `Input*` type carries a filename or a MIME type.** The recipient sees the local
  path's BASENAME, so the caller must materialise each file under the name it should arrive as;
  `tg` renames nothing and cannot. `disable_content_type_detection: true` on documents is what
  stops Telegram re-reading a `kind: file` and showing it as a photo — the caller's `kind` decides
  how the message looks, so it must not be re-derived from the bytes.
- **TDLib groups only same-typed contents into an album**, so a request mixing `photo` and `file`
  is refused rather than split into two messages. One socket request is one Telegram message,
  which is what makes a caller's retry safe.
- **The media path's confirmation semantics are NOT the text path's**, and that is the point.
  `sendMessage` with a local file returns a *pending* message id and uploads afterwards; the text
  path's "timeout returns Ok with the temporary local id" would report a delivery that may never
  happen, and a caller that records that id has already filed the send as done. So
  `send_media_message` waits (`MEDIA_SEND_TIMEOUT_SECS` = 300 — it bounds an upload, not a round
  trip) for `MessageSendSucceeded` on EVERY element, and an expired deadline is an ERROR, never an
  Ok carrying a temporary id. `await_send_confirmations` is the seam that holds this, and it treats
  a `broadcast` `Lagged` as "keep waiting": the text send loop conflates `Lagged` with a closed
  channel, which would fail a send whose confirmation is still in the buffer — do not copy that
  loop into a new path. The text path's own semantics are deliberately unchanged; other callers
  depend on them.

**A partial album is REPORTABLE, not an error (since 0.6.0):** `send_message_album` queues N
INDEPENDENT TDLib messages, each confirmed by its own `updateMessageSendSucceeded` /
`updateMessageSendFailed`, so "the send failed" is not a fact about *the message*. Elements already
confirmed are in the recipient's chat and nothing takes them back. Collapsing that to a flat
`ok:false` is what made the caller record the whole card failed and its retry resend EVERY element
— the recipient got the delivered photos twice, which is the at-most-once guarantee broken by the
one path where it cannot be repaired. Four facts hold the fix together:

- **`await_send_confirmations` no longer returns on the first `MessageSendFailed`.** The other
  elements are still uploading and that update says nothing about them, so the loop keeps waiting
  until every element is resolved or the deadline expires, then reports the whole picture. Pinned
  by `media_wait_keeps_waiting_after_the_first_failure` — reintroducing the early return fails five
  tests.
- **Three per-element states, not two**, because the actions they call for are opposite:
  `delivered` (`{index, message_id}`) must NEVER be resent, `failed` (`{index, error}`) is the only
  class safe to resend, and `unconfirmed` (bare indices — deadline expired or channel closed) is
  neither: TDLib is still uploading, so a blind retry there is a possible duplicate rather than a
  repair. `summarise` projects the loop's `ElementState`s into `MediaElements`, the three lists are
  disjoint, and together they cover every element of the request — which is what makes "not in
  `delivered`" a complete answer to what still has to be sent (`assert_covers`).
- **A failure carries the record, and the prose leads with it.** `TgError::PartialSend` is the one
  error in the enum with structured data on it, and `serve::dispatch` (via `error_response`) answers
  it as `{"ok": false, "error": ..., "result": <partial>}` — the protocol's one failure with a
  `result`. `ok` keeps its meaning, so no caller is told a failure succeeded; what it gains is
  learning which elements arrived instead of assuming none did. The human half of the same fact is
  the message text, which says `ALREADY DELIVERED, do not resend: files[0] ('a.jpg') as message 900`
  BEFORE it says what failed — both come from the one `states` slice, so they cannot disagree. Every
  path that has already handed an element to TDLib fails this way, including the two album
  half-acceptances (`partly_queued_states`: what TDLib queued is `unconfirmed`, what it refused is
  `failed`). A failure that reached Telegram with NOTHING queued stays a bare `ok:false`, so the
  presence of `result` keeps meaning "this is what it nonetheless did".
- **The result field is additive and absent for a text send.** `serve_client::send_request`
  deserialises `SendResult` with a bare `serde_json::from_value`, so `elements` is
  `Option` + `skip_serializing_if`: a text send's JSON stays exactly `{message_id, chat_id}`
  (`a_text_send_result_carries_no_per_element_data`) and an old daemon's answer still parses. The
  failure payload is a separate `PartialSendResult` with no `message_id` at all — there is no single
  id for a send that did not complete, and inventing one is the laundered success this path exists
  to avoid.

**One file is not a one-element album.** TDLib's `sendMessageAlbum` carries 2-10 contents and
refuses one, so `is_album(count) = count > 1` picks the plain `sendMessage` — which is also what
makes a caller's retry of a single remaining element work with no special case at either end
(`a_single_file_is_not_an_album`, `a_retry_of_the_one_remaining_file_is_a_single_file_send`).

**Sending a voice note (`files[].kind: "voice"`, since 0.18.0):** for Tavola's voice recording
(tavola-qdwo), through Postmaster's send; ticket tg-a2e. The contract is in `README.md` → *A voice
note*. What the code relies on, read from `td_api.tl` (tdlib-rs 1.3.0) and Telegram's clients, not
measured: **no test and no agent sends a real voice note.**

- **`inputMessageVoiceNote.waveform` is `bytes`, which tdlib-rs types as `String`**: TDLib's JSON
  carries every `bytes` field in standard base64 with padding. `VoiceNote::tdlib_waveform` is the
  one writer. `tdlib_reads_the_waveform_as_its_forty_bytes` hands it to TDLib's offline
  `testCallBytes`, which answers before `setTdlibParameters` like the static requests above and
  echoes the bytes it decoded, so the same string returning proves TDLib reads our base64.
- **The 5-bit layout is the clients', not TDLib's**: TDLib passes the waveform to Telegram as
  given. Value `i` sits at bit `5 * i` of a little-endian stream (tdesktop's
  `documentWaveformEncode5bit` ORs a `uint16` at byte `5i/8` shifted by `5i%8`; Android's
  `getWaveform2` the same with an `int32`), so 64 levels are 40 bytes and the first level is the
  low 5 bits of byte 0. Telegram's apps make 100 values; any count reads, the apps resample.
  Levels 0-100 are rounded to 0-31 (`five_bit`). The test vectors (silence, full loudness, two
  ramps) were computed by a separate implementation of tdesktop's encoder.
- **Nothing is probed or transcoded**: the image has no ffmpeg. `duration` and `waveform` are the
  caller's, both required, so no voice note goes out as `0:00` with a flat line.
- **Never in an album**: `check_voice_alone` refuses a voice entry beside any other file, in
  `validate_files` and again in `send_media_message` (the trait is public).
- **It reads back as a voice note**: TDLib keeps the content it was given, so the sent message is
  `MessageVoiceNote`, which `extract_message_data` lists as `content_type: "voice"` with
  `[Voice: m:ss]` (or the caption) and `duration_seconds`, as Mycelium reads every other voice note.
