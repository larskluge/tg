# AGENTS.md

This file provides guidance to agents when working with code in this repository.

## Build Commands

```bash
cargo build                    # Build debug
cargo build --release          # Build release
cargo test                     # Run all tests
cargo test cli::tests          # Run CLI tests only
cargo test <test_name>         # Run a single test
cargo clippy                   # Lint
cargo fmt                      # Format
cargo run -- <args>            # Run with args (sets up library path)
cargo run --release -- <args>  # Run release build
```

## Release Build

```bash
make release              # Build release and copy library
./target/release/tg search "test"

make install              # Install to /usr/local/bin (may need sudo)
```

The binary uses `@executable_path/../lib` rpath, so the structure is:
```
target/
├── lib/libtdjson.dylib
└── release/tg
```

## Environment Variables

`tg` reads three optional `TG_*` vars (all prompted interactively by `tg auth` if not set):

- `TG_API_ID` — Telegram API ID from `my.telegram.org` (must be a number)
- `TG_API_HASH` — Telegram API hash from `my.telegram.org`
- `TG_PHONE` — Phone number in E.164 format (e.g. `+1234567890`)

Example setup (optional, for non-interactive use):

```bash
export TG_API_ID=123456
export TG_API_HASH=0123456789abcdef0123456789abcdef
export TG_PHONE=+1234567890
```

Notes:
- `tg auth` uses credentials from: env vars → stored credentials → interactive prompt (in that order).
- On successful `tg auth`, `tg` persists API credentials under `dirs::data_dir()/tg/credentials.json`.
- Non-auth commands (e.g. `tg groups`) read persisted credentials and do not require `TG_API_ID`/`TG_API_HASH`.

## Authentication Flow

Session data is stored in `dirs::data_dir()/tg` (typically `~/Library/Application Support/tg` on macOS and `~/.local/share/tg` on Linux).

Typical first-time auth:

```bash
tg auth
```

What happens during `tg auth`:
1. CLI prompts for API ID and API hash (unless available from env vars `TG_API_ID`/`TG_API_HASH` or stored credentials).
2. CLI prompts for phone number (unless set via `TG_PHONE` env var).
3. CLI prompts for the Telegram verification code.
4. If 2FA is enabled, CLI prompts for the password.
5. On success: `Authenticated successfully!`

Outcome:
- `tg` stores authenticated session state under `dirs::data_dir()/tg` (for example `~/Library/Application Support/tg` on macOS).
- `tg` also stores API credentials in `dirs::data_dir()/tg/credentials.json`.
- Most subsequent commands can use that saved session and saved credentials without re-running `tg auth` or setting environment variables.

If there is already a pending login state (for example waiting for code/password), run `tg auth` again to continue the prompts.

## CLI Examples

```bash
tg auth
TG_API_ID=123456 TG_API_HASH=0123456789abcdef0123456789abcdef TG_PHONE=+1234567890 tg auth
tg chats [--limit 50] [--json]
tg groups [--limit 50]
tg unread
tg send "John Doe" -m "Hello!"
tg send --id 123456789 -m "Hello!"
tg send --group "Family" -m "Hi all!"
tg send --to @username -m "Hi!"
echo "Hello from stdin" | tg send --to @username   # omit -m to read the body from stdin
tg send --to @username --parse-mode HTML -m "<b>bold</b> and <code>code</code>"
tg send --as @mybot --to @someone --parse-mode HTML -m "<b>Hello</b>"
tg messages "John Doe" [--limit 20] [--since-utc 2026-03-01]
tg messages --chat -1001666847309 [--limit 20] [--since-utc 2026-03-01]
tg download --chat -1001666847309 --message 42 [--output-dir .] [--priority 16]
tg search "John"
tg mark-read "John Doe"
tg mark-unread --id 123456789
echo '{"123": 42, "-1001666847309": 89508544512}' | tg sync [--limit 1000]
echo '{"123": 0, "-1001666847309": 0}' | tg sync --reconcile-days 7
echo '{"123": 42, "-1001666847309": 0}' | tg sync --oldest-first --limit 100
echo '{"123": 42}' | tg sync --oldest-first --report-deleted
tg stream                      # NDJSON events from a running `tg serve`; never starts its own client
```

## Architecture

Telegram CLI client using TDLib via `tdlib-rs` with `download-tdlib` feature.

**Key modules:**
- `cli.rs` - Clap-based CLI definitions
- `credentials.rs` - API credential loading/saving (`TG_API_ID`/`TG_API_HASH` and `credentials.json`)
- `error.rs` - Custom error types using thiserror; use `TgError` variants and `Result<T>` alias
- `client.rs` - TDLib client wrapper with `TelegramClient` trait for mocking
- `media.rs` - `send`'s attachments: the `SendFile` wire mirror, the validated `MediaFile`, and the one validator both go through
- `output.rs` - Dual output formatting (plain text default, JSON with `--json`)
- `commands/` - One file per command (`sync.rs` handles bulk message sync for machine consumers; `serve.rs` holds the socket listener and the subscription loop)
- `serve.rs` - The serve protocol: envelopes, the dispatcher, and the `StreamEvent` frames a `subscribe` connection carries (`StreamEvent::from_update` is the one TDLib-update → event mapping)
- `serve_client.rs` - The CLI's side of the socket: one request per connection, plus `stream` for `tg stream`

**Testing pattern:** Mock `TelegramClient` trait for unit tests. CLI parsing tests use `Cli::parse_from()`. Internal algorithms (e.g. `collect_messages_paginated`, `collect_filtered_chats_from_source`) are extracted as free functions taking a source trait so they can be tested without TDLib.

**Session storage:** `dirs::data_dir()/tg` (typically `~/Library/Application Support/tg` on macOS and `~/.local/share/tg` on Linux), including `credentials.json`

## Auth/Env Troubleshooting

- `API ID must be a number`
  - Enter a numeric value for the API ID prompt.
- `API credentials not found at ...`
  - Run `tg auth` to create the credentials file.
- `Not authenticated. Run 'tg auth' first.`
  - Complete the auth flow, then rerun the command.

**TDLib types:** Functions return enums wrapping types (e.g., `tdlib_rs::enums::Chat::Chat(c)` → `tdlib_rs::types::Chat`). Use helper functions like `unwrap_chat()` in client.rs.

**TDLib `getChatHistory` quirk:** May return fewer messages than `limit` on the first call while syncing from the server. Always use a retry+pagination loop: retry on empty responses (up to 5×), and page using the oldest returned message ID as the next `from_message_id`.

**TDLib `getChatHistory` offsets, and walking forward (`tg sync --oldest-first`, since 0.9.0):**
Read from TDLib's source (`OrderedMessages::get_history` in `td/telegram/OrderedMessage.cpp`,
`MessagesManager::get_dialog_history`/`get_history_impl`), because the `td_api.tl` one-liner
("a negative number … to get additionally -offset newer messages") is looser than the code:

- The anchor is the newest message with id `<= from_message_id`. `offset = 0` on an anchor
  that IS `from_message_id` starts strictly OLDER than it — which is why the newest-first
  walker can page with the last id it saw and still dedups defensively.
- `offset = -k` on an existing `from_message_id` spends one of the `k` steps on the anchor
  itself: `k - 1` strictly newer messages come back, then the anchor, then older ones. With a
  deleted `from_message_id` all `k` are strictly newer. So `offset = -1` on an existing message
  returns nothing newer at all — never use it to probe for newer messages.
- Walking off the chat's newest message shrinks the answer (`limit += offset`) instead of
  failing; a local gap makes TDLib load from its database or the server and ask again, and the
  iterator stops at gaps, so the newer messages in an answer are always the run immediately
  after the anchor. That contiguity is what makes "advance the cursor to the newest id
  returned" skip nothing.
- `from_message_id = 0` means "from the last message". To start at a chat's FIRST message,
  anchor at `1 << 20` (server message id 1): TDLib turns it into `messages.getHistory` with
  `offset_id = 1`, Telegram's idiom for reading from the start.

`collect_messages_forward` therefore asks `(from = cursor, offset = -99, limit = 100)` every
time, keeps ids `> cursor`, sorts ascending and stops at `limit`. An EMPTY answer means TDLib is
still loading (retry, like the newest-first walker); a non-empty answer with nothing newer than
the cursor means it answered from the anchor and found nothing after it — caught up, stop at
once. Conflating the two costs every idle chat 1.5 s of retry sleeps per sync, and resetting
the retry count on such an answer loops forever. Stopping early is never lossy: the caller's
cursor only moves past messages it was given. Tested against `WindowedHistory`, a fake that
answers with this window arithmetic (pinned by `windowed_history_models_tdlib_offsets`) rather
than scripted batches.

**A deleted private chat (`tg sync --report-deleted`, since 0.10.0):** TDLib has no "chat
deleted" field, and none of the updates `subscribe` forwards says a chat was deleted, so the
signal is read off the chat's state.
Read from TDLib's source (`MessagesManager::update_dialog_pos`, `get_chat_positions_object`,
`set_dialog_is_empty`, `on_get_history`):

- **`chat.chat_lists`, never `chat.positions`.** `positions` reports a list only while the
  chat lies inside the part of that list loaded so far (`list_last_dialog_date_`), so every
  quiet chat has empty `positions` until something loads the whole list. `chat_lists` is
  membership (`d->dialog_list_ids`), independent of loading: empty exactly when TDLib's
  order for the chat is `DEFAULT_ORDER`.
- A chat whose messages are all gone gets that order: deleting its last message makes TDLib
  re-read the history from the server, and an empty answer from the end marks the chat
  `is_empty`, which drops it from every list and clears `last_message`. So a deleted private
  chat reads: in no chat list, no last message, empty history.
- **The same state appears transiently** for a chat just loaded from the database whose last
  message is not loaded yet ("no known messages in the chat, just leave it where it is" keeps
  the in-memory default order until `load_last_dialog_message` answers). That is why
  `chat_is_deleted` reads `getChat`, then the history from the end (`getChatHistory` loads
  from the database and then the server before it answers empty), then `getChat` again, and
  reports a deletion only if the state held throughout. An error at any step is an error.
- A chat TDLib has never seen answers `getChat` with "Chat not found" — an error, not a
  deletion. That is what a session created after the deletion sees, so the signal only
  works in a session that synced the chat before it was deleted, like `tg serve`'s.
- Measured 2026-09-28 against `tg serve` on outpost, every chat Mycelium syncs at its own
  cursor: 2 of 73 DMs reported `chat_deleted` — the two a local session created later did
  not know at all — and the other 71 DMs and 6 groups read as ordinary chats.
- Only private chats are reported: a basic group or supergroup also leaves every list when
  you leave it, but leaving a group is not deleting a chat.

**TDLib `getChatMessageByDate` direction:** It returns the last message sent **no later than** the given date — the returned message's date is always `<= date` — and a **404** when the chat has no such message. It does not find the first message *after* a date. To turn a `--since-utc` cutoff into a fetch boundary, probe at `cutoff - 1` and use the returned message's `id + 1` as an exclusive lower bound (`boundary_probe_date` / `boundary_from_probe` in `client.rs`). Reading it as "at or after the date" makes the lookup silently never match.

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

**TDLib HTML parse mode is not HTML:** `textParseModeHTML` accepts only Telegram's tag
whitelist — `b`/`strong`, `i`/`em`, `u`/`ins`, `s`/`strike`/`del`, `a href`, `code`, `pre`
(+ `code class="language-x"`), `blockquote` (optionally `expandable`), `tg-spoiler`,
`tg-emoji`. There are no headings, lists, `hr`, `p` or `br`. TDLib **errors** on any tag
outside the set (it does not strip it), so `<h1>x</h1>` fails the send rather than degrading.
Tag names are case-insensitive. Only `&`, `<`, `>` need escaping; escape all three
unconditionally, never selectively. Related facts, all measured against TDLib 1.8.61:

- `parse_text_entities` returns `enums::FormattedText`, so it needs `unwrap_formatted_text()`
  before it can go into `InputMessageText` (see **TDLib types** above). It also takes an
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

**Testing against real TDLib:** static requests (`parseTextEntities`, `getMarkdownText`,
`setLogVerbosityLevel`) answer on a client that has never called `setTdlibParameters`, so a
test needs `tdlib_rs::create_client()` and a receive thread and nothing else — no credentials,
no database files, no network. tdlib-rs 1.3.0 exposes no synchronous `td_execute`, so the
receive loop is mandatory even for a static request. Three rules, all learned by hitting
them, and all three abort the process rather than failing a test:

- **Exactly one receive loop per process.** TDLib aborts ("Receive must not be called
  simultaneously from two different threads") as soon as `cargo test` runs two loop-owning
  tests in parallel. `client::tdlib_parse_tests` shares one refcounted loop behind a mutex.
- **Join the loop; never leak it.** Hold the lock across the join, or a replacement loop
  starts while the old one is still parked inside `td_receive`.
- **Close the client, and wait for `authorizationStateClosed`.** This is the one that bites
  silently. TDLib frees a client's resources only after that update — "All resources will be
  freed only after authorizationStateClosed has been received" — and a process that exits
  with a client still open aborts inside TDLib's teardown, *after* the harness has already
  printed `test result: ok`. The symptom is a bare `SIGABRT` with no failing test and, because
  the harness sets log verbosity to 0, no TDLib message either. It is intermittent, so it
  passes locally and on the PR run and then fails on `main` — where it blocks the image push.
  Measured by looping the real test binary on 4 cores against libtdjson 1.8.61: 15 of 100 runs
  aborted without the close, 0 of 100 with it.

So teardown is: send `close`, let the loop keep pumping until it sees `Closed`, then join. The
loop's exit condition is that update, not a shutdown flag, and both the close acknowledgement
and the `Closed` confirmation are asserted — losing either means the process is once again
exiting with a live client, and that must fail where it is legible rather than at exit.

Set log verbosity with the synchronous `set_tdlib_log_verbosity` **before** `create_client()`.
The async `setLogVerbosityLevel` only takes effect once TDLib is already up, so it cannot
suppress the startup banner, which otherwise floods CI logs.

**Subscriptions (`subscribe` / `tg stream`, since 0.11.0):** the frames, names and exit codes
are a contract with Mycelium's Telegram consumer (its spec:
`docs/specs/2026-09-30-realtime-message-streaming-design.md` in mycelium) and are documented in
`docs/specs/2026-05-22-serve-design.md` → *Event frames*. What the code relies on:

- **A subscription never takes the client lock.** `serve::run` takes the update channel
  (`TdLibClient::updates()`) before the client goes behind its mutex and hands each connection
  a `Subscriptions`. `handle_connection` answers `subscribe` itself, so `serve::dispatch` never
  sees it (it refuses the command outright if called directly). Pinned by
  `subscribe_acks_then_streams_events_without_the_client_lock`, which holds the lock throughout.
- **Attach, then ack.** The broadcast receiver is created before the ack is written, so nothing
  after the ack can be missed.
- **Shutdown ends subscriptions first**, through a `watch` switch. A subscription never ends on
  its own, so without it every live subscriber held the drain for its full 5 s, on top of
  TDLib's close. Measured locally with one subscriber: `tg serve` stops in 2.1 s.
- **The peer's close is read, not written.** A subscription reads (and discards) the peer's
  input to see EOF, so a half-close ends it: `serve_client::stream` keeps its write half alive
  for exactly that reason. A peer that vanishes without a close is found by the heartbeat write.
- **The channel is lossy by design.** `UPDATE_CHANNEL_CAPACITY` (1024) bounds how far a
  subscriber may fall behind; past it tokio drops updates for that receiver alone, the sender
  never blocks, and the subscriber gets `lagged`. The short-lived receivers (auth, send
  confirmations) share the same channel and gain from the larger capacity.
- **Loading chats rings every chat once.** The first `chats`/`groups`/`unread` request of a
  serve session makes TDLib announce each loaded chat's last message: ~405
  `chat_last_message` frames from one `tg chats --limit 2` on 2026-09-30, and none from the
  requests after it.
- **Supergroups and channels may not ring.** TDLib's `openChat` docs: "all updates are received
  only for opened chats" in them. `tg` does not call `openChat`; how many synced groups ring
  anyway is to be measured on outpost.
- **`tg stream` has no in-process fallback**, unlike every other command: a second TDLib client
  on serve's database is the one thing it must never open. It exits 1 when serve is
  unreachable or ends the stream, and 0 when its stdout closes **or its stdin reaches EOF**.
- **Stdin's EOF is the lifetime signal under `podman exec -i`**, not stdout. Measured with
  podman 6.1.2: when the exec client goes away (reader closed, SIGTERM, SIGKILL), the process in
  the container keeps running because conmon keeps accepting its stdout, but its stdin is closed
  in every case. `serve_client::stdin_closed` reads stdin on a plain, never-joined thread:
  tokio's `stdin()` cannot be cancelled and would hold the runtime's shutdown — and so the exit
  after `tg serve closed the stream` — until stdin closed.

**Serve request strictness:** `SendRequest` and its nested `media::SendFile` are the only serve
request structs carrying `#[serde(deny_unknown_fields)]`. The others are deliberately open: `WhoamiRequest{}` backs
the container's `HealthCmd` (`tg whoami`, `HealthStartupTimeout=2m`), so tightening it risks
the health gate for no benefit, and the remaining structs have caller sets that were never
audited. `dispatch_other_commands_still_ignore_unknown_args` pins the decision so a later
blanket change has to be deliberate.

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

An explicit `"parse_mode": null` is accepted and means plain text, exactly like an absent
key. `"files": null` is accepted the same way and for the same reason. That asymmetry with `""`
(refused) is deliberate: `null` is what an unset optional
serialises to in Go (`map[string]any` with a missing key, or a `*string` without
`omitempty`), in Python (`json.dumps`) and in `tg`'s own CLI proxy, so refusing it would
turn every plain-text send from those callers into a hard error while closing nothing — a
caller that omits the key entirely still gets a plain send, and "absent means plain" is the
contract. Pinned by `send_request_accepts_explicit_null_parse_mode`.

**Clap negative IDs:** Telegram supergroup IDs are negative (e.g. `-1001666847309`). Any `--id` arg that accepts `i64` needs `#[arg(long, allow_hyphen_values = true)]` or clap will treat the leading `-` as a flag.
