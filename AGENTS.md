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
tg message --chat -1001666847309 --message 89508544512 [--json]   # one message, reactions included
tg react --chat -1001666847309 --message 89508544512 👍 [--remove]  # NEVER against a real chat from a test or an agent
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
- `reactions.rs` - Pure functions over TDLib's reaction types: `listed_reactions` (the ONE reader of `interaction_info.reactions`), `telegram_form` (which spelling of an emoji Telegram takes), `chosen_types` (what to take back)
- `reaction_changes.rs` - The reaction changes Telegram has not answered yet, per message (`ReactionChanges`): what `react` waits on before it asks again
- `commands/` - One file per command (`sync.rs` handles bulk message sync for machine consumers; `message.rs` re-reads one message; `react.rs` adds or takes back a reaction; `serve.rs` holds the socket listener and the subscription loop)
- `reaction_poll.rs` - `tg serve`'s poll of group reactions: which groups and messages, the tiers, the guard against a drop in unread counts, and the round loop (`Poller`); `PollSource` is what it asks of TDLib, implemented for `TdLibClient` in `client.rs`
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

**A channel is a supergroup with `is_channel` (`is_channel` on every chat row, since 0.13.0):**
TDLib has no chat type of its own for a channel. `chat.type` is `chatTypeSupergroup
{ supergroup_id, is_channel }` for a channel and for every other supergroup, and the flag is the
whole difference ("True, if the supergroup is a channel"); a forum and a broadcast group are
flags on the `supergroup` object, not on the chat type. Read from the `td_api.tl` tdlib-rs 1.3.0
pins, not measured: no test and no agent lists a real account's chats.

- **`chat_type_kind` is the one place the flag is read.** It yields `ChatTypeKind::Channel`, and
  `ChatSnapshot::to_chat_info` turns that into `is_channel` on `ChatInfo`, the row `chats`,
  `groups` and `unread` share, in `--json` output and in a `tg serve` result alike.
- **Always a boolean.** `is_bot` is `Option<bool>` because it takes a `getUser` that can fail;
  `is_channel` sits on the `chat.type` that `fetch_chat_snapshot` already holds, so it costs no
  request and is never unknown. Its `#[serde(default)]` is for `tg`'s own CLI reading rows from a
  `tg serve` older than 0.13.0: a missing key reads `false`.
- **`groups` lists channels, as it always has** (`is_group_chat`: basic group, supergroup,
  channel), and `chats` leaves them out (`is_private_chat`). The row is a contract with Mycelium
  (its spec: `docs/specs/2026-10-12-channel-kind-design.md` in mycelium), which discovers
  Telegram's channels through `groups` and labels a row `channel` when the key is `true`, `group`
  when it is `false` or absent. Taking channels out of `groups`, or giving them a command of
  their own, would end that discovery. There is no filter flag, and the table and plain-text
  output do not show the key.

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
- **A new shape does not rewrite what Mycelium already holds.** It keys a Telegram message on
  `tg:<chat>:<message>:<edit_date>` and records a message again only when that changes, so a
  message ingested before the change keeps its old body until it is edited.

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
  only for opened chats" in them. Only the reaction poll calls `openChat`, for a moment per
  group and interval (see **Reactions** below), and channels never; how many synced groups ring
  in between is to be measured on outpost.
- **`tg stream` has no in-process fallback**, unlike every other command: a second TDLib client
  on serve's database is the one thing it must never open. It exits 1 when serve is
  unreachable or ends the stream, and 0 when its stdout closes **or its stdin reaches EOF**.
- **Stdin's EOF is the lifetime signal under `podman exec -i`**, not stdout. Measured with
  podman 6.1.2: when the exec client goes away (reader closed, SIGTERM, SIGKILL), the process in
  the container keeps running because conmon keeps accepting its stdout, but its stdin is closed
  in every case. `serve_client::stdin_closed` reads stdin on a plain, never-joined thread:
  tokio's `stdin()` cannot be cancelled and would hold the runtime's shutdown — and so the exit
  after `tg serve closed the stream` — until stdin closed.

**Serve request strictness:** `SendRequest`, its nested `media::SendFile` and `ReactRequest` —
the requests that put something in a chat — are the only serve request structs carrying
`#[serde(deny_unknown_fields)]`. The others are deliberately open: `WhoamiRequest{}` backs
the container's `HealthCmd` (`tg whoami`, `HealthStartupTimeout=2m`), so tightening it risks
the health gate for no benefit, and the remaining structs have caller sets that were never
audited. `dispatch_other_commands_still_ignore_unknown_args` pins the decision so a later
blanket change has to be deliberate.

**Reactions (since 0.12.0):** a listed message carries `reactions`, `subscribe` rings
`message_reactions`, `message` re-reads one message, `react` adds or takes back. The shapes are
a contract with Mycelium (its `message_reactions` projector) and Postmaster (`send_reaction`),
documented in `README.md` → *Reactions*, *`react`*, *One message*. All of the following was read
from TDLib's source at the commit tdlib-rs 1.3.0 pins (`11e254af`, 1.8.61) and from Telegram's API
docs, not measured: **no test and no agent ever sends a reaction to a real chat.**

- **Measured read-only on 2026-10-06** (a local session, listing only, counts only): 2,669
  messages of 37 chats, 389 with reactions in 438 entries; no emoji with U+FE0F in any of them;
  the account never among `recent_senders`; in all 232 entries of one-to-one chats
  `count == chosen + recent_senders.len()`; never more than three senders; 32 entries with an
  uncredited rest. `getMessageAvailableReactions` offered 73 emoji for a group message and for a
  private one, none with the selector, and `telegram_form` found the bare heart for a keyboard's.
  `getMessage` for an id the chat does not hold answers `Not Found`. On outpost's `tg serve` the
  same day, after the roll to 0.12.0: 1,956 messages of 35 chats, 274 with reactions in 280
  entries, and ONE emoji with U+FE0F — U+1F441 U+FE0F in a group, which is not a reaction
  Telegram offers, so a custom emoji listed as its sticker spells it. Telegram's own reactions
  carry no selector; what `tg` lists can.
- **`reactions::listed_reactions` is the one reader of `interaction_info.reactions`**, as
  `reply_in_chat` is of `reply_to`. It drops paid reactions, lists a custom emoji under the
  emoji `getCustomEmojiStickers` names for it (one call per batch of messages, cached by TDLib,
  5 s at most; unnamed ones are left out with a line on stderr), merges two TDLib reactions that
  are one emoji once variation selectors are set aside, and keeps the account out of the senders.
- **`used_sender_id` is how the account is recognised among `recent_sender_ids`.** TDLib sets it
  exactly when the reaction is chosen AND the account's own sender is among the recent choosers it
  returns (`MessageReaction::get_message_reaction_object`): always in a private chat, where it
  builds the list itself (the account if chosen, then the peer if the count leaves room), and in
  a group only when Telegram named the account among the recent three. So "drop the sender equal
  to `used_sender_id`" is exact, and `count - chosen - recent_senders.len()` is the uncredited
  rest. `recent_senders[].id` is `null` for a chat sender, as `sender_id` is on a message.
- **TDLib compares a reaction to the offered ones byte for byte.** `ReactionType` keeps the string
  as given and `addMessageReaction` answers 400 `The reaction isn't available for the message`
  unless `ChatReactions::is_allowed_reaction_type` finds it. Telegram's forms carry no U+FE0F (its
  heart is U+2764), a keyboard's do. `react` therefore reads `getMessageAvailableReactions` first
  and sends the offered emoji that equals the asked one without variation selectors
  (`telegram_form`); none found, it sends the emoji as given so the refusal is Telegram's own.
  The three lists of `availableReactions` together hold every offered reaction
  (`get_sorted_available_reactions` ends in `CHECK(all_available_reaction_types.empty())`), and
  the request is answered from TDLib's memory. Do not replace this with "strip U+FE0F and retry
  on that error text": it works today only because Telegram happens to offer no emoji with the
  selector.
- **`addMessageReaction` changes TDLib's own copy first.** `set_message_reactions` sends
  `updateMessageInteractionInfo` and only then `messages.sendReaction`; the request's answer is
  Telegram's. On an error TDLib queues a reload of the message's reactions. So: a `react` through
  `tg` rings a `message_reactions` frame by itself; a read-back right after an ERROR may still
  show the reaction (which is why `react` reads back only after success); and a request that
  times out says nothing about the outcome. Reactions are not in TDLib's binlog ("TODO … log
  event"). An account without Premium has one reaction per message, and adding the same one again
  changes nothing on Telegram.
- **TDLib answers a repeat from its own copy, so `tg` keeps what Telegram has not answered (since
  0.15.1, tavola-fkp.6).** A second `addMessageReaction` for a reaction the copy already shows is
  answered ok at once, without asking Telegram (`MessagesManager::add_message_reaction`:
  `add_my_reaction` returns false for one already chosen and not big, and the promise is set), and
  `removeMessageReaction` likewise for one it no longer shows; `react`'s read-back reads that same
  copy. So a request that stopped waiting after 10 s did not end its change: `ReactionChanges`
  runs every reaction call on a task of its own and keeps the change, per `(chat_id, message_id)`,
  until Telegram answers (`Settled` takes it off however the task ends). `react::handle` asks
  `earlier_reaction_change` before anything else and waits up to 10 s more: still unanswered, it
  fails and asks nothing; refused, a repeat of that change (`ReactionChange::repeats`: the same
  emoji in either spelling, any taking back for a taking back of everything) gets the refusal,
  since TDLib's copy shows the refused change until its reload lands; otherwise it goes ahead. The
  mock models TDLib the same way (its copy first, a silent Telegram on `telegram_answers`), so the
  tests drive the real `ReactionChanges`. What it cannot cover: a process that stops forgets what
  it asked, and a new one's copy may show a change Telegram never got.
- **`removeMessageReaction` takes back exactly the type it is given** and answers ok, having done
  nothing, for one the account has not chosen. What `tg` lists as one emoji can be a custom emoji,
  so `remove_message_reaction` reads the message and takes back `reactions::chosen_types`.
- **`message_reactions` rings on every `updateMessageInteractionInfo`**, with or without an
  `interaction_info`: the last reaction taken back leaves none, and that update is the only sign
  of it. The same update carries a channel post's view and forward counters and a discussion
  thread's reply count, and an update holds only the new state, so telling those apart would
  take a cache of what each message had. The consumer compares instead.
- **Which reaction changes reach TDLib on their own** — what Telegram pushes, whether or not a
  chat is open:
  - *One-to-one chats:* every change. TDLib never polls there
    (`need_poll_dialog_message_reactions` is `false` for `DialogType::User`).
  - *Basic groups and supergroups:* a reaction to the account's OWN message (Telegram's API docs:
    "Message authors will receive an updateMessageReactions update when a user reacts to their
    message", subject to the account's reaction-notification setting), and the account's own
    reaction made through `tg`. Whether the account's own reaction made on another device arrives
    is not established. A reaction between other people is not pushed: Telegram's docs tell apps
    to "short-poll reactions for visible messages (that weren't sent by the user) once every 15-30
    seconds", and TDLib does that only for messages reported with `viewMessages` while the chat's
    `open_count > 0`. Hence the poll below.
  - *Reading again does not help.* `getChatHistory` returns what `ordered_messages` holds without
    asking Telegram (`get_dialog_history`), and `getMessage` asks only for a message TDLib does
    not have. So `sync --reconcile-days` and `message` answer with TDLib's copy, and a consumer's
    reconcile catches missed doorbells, not reactions TDLib never received.

**The reaction poll (`tg serve`, since 0.15.0; `reaction_poll.rs`).** `tg serve` does what a
Telegram app does for the messages on its screen: every group whose last message is at most seven
days old is opened (`openChat`), its newest messages of those seven days (at most 100, read from
TDLib's own copy with `only_local`, never a live location) are reported with `viewMessages`, and it
is closed (`closeChat`). TDLib then asks Telegram for their reactions, and every change rings as an
ordinary `message_reactions` frame: no consumer changes anything. All of the following is read from
TDLib's source at `11e254af` (1.8.61; `td/telegram/…`), so **check it again when tdlib-rs moves to
another TDLib**:

- **The source is the whole risk, and it is `messageSourceNotification`, `force_read: false`.**
  `view_messages` (`MessagesManager.cpp:15959`) turns `Auto` into `DialogHistory` while the chat is
  open (`:15965`), and only a history source or `force_read` sets `need_read` (`:15974`). Everything
  that marks something read waits on it: the unread mentions and voice/video contents
  (`read_message_contents_on_server`, `:16158`), the unread reactions to the account's messages
  (`read_message_reactions_on_server`, `:16164`) and the inbox (`read_dialog_inbox`, `:16328`, after
  `if (!need_read) return` at `:16261`). `messageSourceOther` would do too, but it also queues
  `messages.getMessagesViews` for a message with views (`need_update_view_count`, `:15975`; without
  the view-counter increment, which only history sets, `:16225`) and marks downloads viewed in the
  download manager (`:15977`). A notification sets neither, so it is the source with the fewest side
  effects that still polls. The default source in an open chat MARKS THE MESSAGES READ: never pass
  `None`.
- **A live location is never reported.** Viewing an active live location in an open chat, from any
  source, sends `readMessageContents` for it (`on_message_live_location_viewed`, `:16153` →
  `:18086`). `is_live_location` leaves out every `messageLocation` with a `live_period`.
- **What polls.** A viewed server message of an open chat goes into `dialog_viewed_messages_`
  (`:16187`) and through `process_viewed_message` (`:9584`), which queues a reaction reload for the
  ones `need_poll_message_reactions` picks (`:6086`): a message TDLib holds reactions for, or one
  last updated under another generation of the chat's available reactions. A message nobody reacted
  to is not polled: Telegram sends a message's first reaction as an edit of it (tdesktop's poll says
  so: "First reaction always edits message"; not read in Telegram's own docs), which reaches TDLib
  with the chat's updates, and `openChat` on a supergroup fetches those (`get_channel_difference`,
  `:16486`). `queue_message_reactions_reload` (`MessageQueryManager.cpp:2467`) sends one
  `messages.getMessagesReactions` per 100 messages at once; the answer's `updateMessageReactions`
  updates the message, and a message the answer leaves out is set to no reactions (`:1008`).
- **`closeChat` right after does not cancel it.** `close_dialog` (`MessagesManager.cpp:16534`)
  drops `dialog_viewed_messages_` (`:16576`) and the repeat timer, but the reload queue
  (`being_reloaded_reactions_`) is `MessageQueryManager`'s and is left alone. The repeat timer would
  not run anyway: it needs the account online (`:16251`, `:9574`, `:9617`), and `tg` never sets
  the `online` option (`OnlineManager.h:52`: offline by default), so the poll also never shows the account
  online. Each poll is exactly one reaction reload per chat.
- **What `openChat` itself costs**, all reads (`open_dialog`, `:16420`): a basic group's full info
  (`repair_chat_participants`) and peer settings; a supergroup's difference, peer settings, full
  info when it is out of date, and its 200 recent members when it has fewer than 195; the pinned
  message and a group call's state when not cached; and the chat goes onto TDLib's local list of
  recently opened chats. Roughly 3 to 5 requests per open, plus the reaction reload. Reporting
  also reloads a fact check, a paid medium or a forwarded story where a message carries one.
  Reading the history with `only_local` waits on no request, though TDLib may preload the
  neighbouring history in the background, as it does for every history read. Listing the chats
  (`getChats` for Main and Archive) loads the lists from Telegram the first time, once per
  session, and holds the client lock meanwhile.
- **The guard.** Each poll reads the chat's read state (`unread_count`, `unread_mention_count`,
  `unread_reaction_count`, `is_marked_as_unread`, `last_read_inbox_message_id`) before
  `openChat` and again after `viewMessages`, before `closeChat`. TDLib changes it synchronously
  inside `view_messages` when it reads anything (and `open_dialog` reads locally only in a chat
  without a last message, `:16440`, which the poll never opens), while what `openChat`'s requests bring back (a read made on another device) lands
  later. A count that falls, the mark that goes or the inbox's read mark moving up (which no new
  message can hide, as one can hide a lower count) stops the poll until `tg serve` restarts
  (`Stop::Dropped`), with one line on stderr; so does a read state that cannot be read
  afterwards (`Stop::Unverified`). A read made on another device inside those few milliseconds
  would stop it too. A chat is closed on every path once opened; one whose `closeChat` failed is
  closed again before the next round and when the poll ends.
- **Tiers.** By the age of a group's last message: at most an hour, every 5 minutes; a day, every
  20; seven days, every 60. A round starts every minute and polls what is due, the most recently
  active first, one chat at a time with 2 s between them; a round never overlaps the next, and an
  interval runs from the round a chat was polled in. Only a basic group or a non-channel supergroup
  of the Main or Archive list that allows reactions is polled; the first round comes a minute after
  start. The poll holds the client lock for one chat's reads and calls at a time (local, a few
  milliseconds), so a send waits at most that; a round reads every listed chat's state, one
  `getChat` each, to find the groups that are due.
- **`TG_REACTION_POLL=off`** turns it off; any value but `on` or `off` refuses to start `tg serve`.
- **Logs are numbers only:** the first round's counts (groups in the window, polled, reported,
  with reactions, and the unread totals before and after), a summary every hour (rounds, polls,
  frames rung within 10 s of a poll, the unread totals now), each distinct error once, and the stop.
- **A reaction found by a poll is dated late by the consumer.** Mycelium dates a reaction it hears
  of by the ring. A ring from a poll comes up to one interval after the reaction was made; the first
  poll after the roll found everything TDLib had missed until then.

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
