# tg

A Telegram CLI on TDLib (`tdlib-rs` with its `download-tdlib` feature), and `tg serve`, the
long-lived server other programs reach over its socket: Mycelium reads chats, messages and live
events from it, and Postmaster sends, reacts and deletes through it. It runs on outpost as the
`telegram` container (`ghcr.io/larskluge/tg:latest`).

`CLAUDE.md` is a link to this file: edit `AGENTS.md`. It holds the rules every change keeps; how each
feature works is in [docs/architecture/](docs/architecture/README.md), and "Docs" below says what
goes where.

## Tracking work

Work is tracked in Beads (`bd`): run `bd prime`, then `bd ready`, and file new work with
`bd create`, never `gh issue create` (GitHub Issues stay on, but are not where work is tracked; the
GitHub issues moved here as `tg-gh<N>`, keeping their numbers). Storage, the Dolt remote on outpost
(`refs/dolt/data`) and recovery: `.beads/README.md`.

## Layout

**Key modules:**
- `cli.rs` - Clap-based CLI definitions
- `credentials.rs` - API credential loading/saving (`TG_API_ID`/`TG_API_HASH` and
  `credentials.json`)
- `error.rs` - Custom error types using thiserror; use `TgError` variants and `Result<T>` alias
- `client.rs` - TDLib client wrapper with `TelegramClient` trait for mocking
- `media.rs` - `send`'s attachments: the `SendFile` wire mirror, the validated `MediaFile` with its
  `Presentation` (a photo's dimensions, a voice note's length and waveform, nothing for a
  document), the one validator both go through, and the voice note's 5-bit waveform packing
- `marks.rs` - A message's formatting as `marks`: TDLib's entities turned into the five kinds a
  reader draws, at UTF-8 byte offsets of the `text` tg writes
- `parse_mode.rs` - `ParseMode`, the formatting mode `tg send` carries on the CLI, the socket and
  the Bot API
- `bot_api.rs` - The Telegram Bot API calls behind `tg send --as @bot`
- `resolve.rs` - A send's recipient, resolved
- `output.rs` - Dual output formatting (plain text default, JSON with `--json`)
- `reactions.rs` - Pure functions over TDLib's reaction types: `listed_reactions` (the ONE reader
  of `interaction_info.reactions`), `telegram_form` (which spelling of an emoji Telegram takes),
  `chosen_types` (what to take back)
- `reaction_changes.rs` - The reaction changes Telegram has not answered yet, per message
  (`ReactionChanges`): what `react` waits on before it asks again
- `commands/` - One file per command (`sync.rs` handles bulk message sync for machine consumers;
  `message.rs` re-reads one message; `react.rs` adds or takes back a reaction; `delete_chat.rs`
  deletes a one-to-one chat up to a date; `serve.rs` holds the socket listener and the
  subscription loop)
- `reaction_poll.rs` - `tg serve`'s poll of group reactions: which groups and messages, the tiers,
  the guard against a drop in unread counts, and the round loop (`Poller`); `PollSource` is what it
  asks of TDLib, implemented for `TdLibClient` in `client.rs`
- `serve.rs` - The serve protocol: envelopes, the dispatcher, and the `StreamEvent` frames a
  `subscribe` connection carries (`StreamEvent::from_update` is the one TDLib-update → event
  mapping)
- `serve_client.rs` - The CLI's side of the socket: one request per connection, plus `stream` for
  `tg stream`

`tests/serve_integration.rs` runs the serve protocol over real sockets against a stub server;
`tests/agents_md.rs` keeps this file small ("Docs").

**Session storage:** `dirs::data_dir()/tg` (typically `~/Library/Application Support/tg` on macOS
and `~/.local/share/tg` on Linux), including `credentials.json`

## Rules

- **No test and no agent acts on a real chat**: never a send, a reaction, a voice note or a
  `delete_chat` against a real account. What TDLib does is read from its source or `td_api.tl` (at
  what tdlib-rs pins) or measured read-only, and a measurement records numbers only.
- **The output and the serve protocol are contracts** with Mycelium, Postmaster and Tavola: the
  shapes are in `README.md` and `docs/specs/2026-05-22-serve-design.md` (*Event frames*), and change
  with the code. A change is additive: a new key is left out where it has nothing to say, and a
  reader of an older `tg serve` still parses (`#[serde(default)]`, `Option` +
  `skip_serializing_if`).
- **One seam per shape; change it there and nowhere else**: `extract_message_data` writes a
  message's `text` and `marks`, `reply_in_chat` reads `reply_to`, `reactions::listed_reactions`
  reads `interaction_info.reactions`, `chat_type_kind` reads `is_channel`,
  `StreamEvent::from_update` maps TDLib updates to frames, `VoiceNote::tdlib_waveform` writes a
  waveform.
- **Entity offsets come from TDLib**: always `parse_text_entities`, never hand-rolled UTF-16
  offsets; under `HTML`, escape `&`, `<` and `>` unconditionally
  ([sending.md](docs/architecture/sending.md)).
- **A media send waits for Telegram**: an expired deadline is an error, never an Ok with a
  temporary id, and a partial album answers per element (what is `delivered` is never resent). The
  text path's semantics stay as they are: other callers depend on them
  ([sending.md](docs/architecture/sending.md)).
- **The requests that change a chat are strict**: `SendRequest`, `media::SendFile`, `ReactRequest`
  and `DeleteChatRequest` carry `#[serde(deny_unknown_fields)]`; the others stay open on purpose
  (`WhoamiRequest` backs the container's health check; [serve.md](docs/architecture/serve.md)).
- **`tg stream` never opens a TDLib client of its own**: a second client on serve's database is
  the one thing it must never open.
- **The reaction poll reports with `messageSourceNotification`, `force_read: false`**, never the
  default source (in an open chat it marks the messages read), and never a live location
  ([reactions.md](docs/architecture/reactions.md)).

**TDLib types:** Functions return enums wrapping types (e.g., `tdlib_rs::enums::Chat::Chat(c)` →
`tdlib_rs::types::Chat`). Use helper functions like `unwrap_chat()` in client.rs.

**Clap negative IDs:** Telegram supergroup IDs are negative (e.g. `-1001666847309`). Any `--id` arg
that accepts `i64` needs `#[arg(long, allow_hyphen_values = true)]` or clap will treat the leading
`-` as a flag.

**Testing pattern:** Mock `TelegramClient` trait for unit tests. CLI parsing tests use
`Cli::parse_from()`. Internal algorithms (e.g. `collect_messages_paginated`,
`collect_filtered_chats_from_source`) are extracted as free functions taking a source trait so they
can be tested without TDLib. A test against a real TDLib shares `client::tdlib_parse_tests`' one
receive loop and closes its client, waiting for `authorizationStateClosed`, or the process aborts
after the tests pass ([docs/build-and-test.md](docs/build-and-test.md)).

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

make install              # Link ~/bin/tg (BIN_DIR) to the release binary
```

The binary uses `@executable_path/../lib` rpath, so the structure is:
```
target/
├── lib/libtdjson.dylib
└── release/tg
```

<!-- BEGIN OWL -->
## Testing (owl tier: ci-gated)

- Before landing: the tests your change touches, plus `cargo test -- cli::tests agents_md` (smoke). Never the full suite in-session.
- One test: `cargo test <test_name>`.
- Full suite: GitHub CI (`ci.yml`). After pushing a change it covers, `gh run watch --exit-status`: the deploy waits on it.
<!-- END OWL -->

## Land

1. **Verify**: the tests your change touches, plus the smoke (above).
2. **Version**: a change to what `tg` does bumps `version` in `Cargo.toml` (and `Cargo.lock`),
   named in the commit subject: `(tg 0.N.0, <ticket>)`. Docs and tests alone bump nothing.
3. **Land**: commit, and from a worktree squash-merge into `main` under `main-lock`, after
   `git pull --rebase origin main`; push `origin main`.
4. **Deploy**: CI (`ci.yml`) runs the tests, then builds and pushes `ghcr.io/larskluge/tg`
   (`latest`, `sha-<sha>`): `gh run watch <id> --exit-status`. outpost's `telegram` container
   rolls `latest` itself (`AutoUpdate=registry` in `~/code/outpost/quadlets/telegram.container`).
5. **Close**: `bd close` the ticket, `bd dolt push`, delete the branch, remove the worktree,
   release the lock.

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
- `tg auth` uses credentials from: env vars → stored credentials → interactive prompt (in that
  order).
- On successful `tg auth`, `tg` persists API credentials under
  `dirs::data_dir()/tg/credentials.json`.
- Non-auth commands (e.g. `tg groups`) read persisted credentials and do not require
  `TG_API_ID`/`TG_API_HASH`.

## Authentication Flow

Session data is stored in `dirs::data_dir()/tg` (typically `~/Library/Application Support/tg` on
macOS and `~/.local/share/tg` on Linux).

Typical first-time auth:

```bash
tg auth
```

What happens during `tg auth`:
1. CLI prompts for API ID and API hash (unless available from env vars `TG_API_ID`/`TG_API_HASH`
   or stored credentials).
2. CLI prompts for phone number (unless set via `TG_PHONE` env var).
3. CLI prompts for the Telegram verification code.
4. If 2FA is enabled, CLI prompts for the password.
5. On success: `Authenticated successfully!`

Outcome:
- `tg` stores authenticated session state under `dirs::data_dir()/tg` (for example
  `~/Library/Application Support/tg` on macOS).
- `tg` also stores API credentials in `dirs::data_dir()/tg/credentials.json`.
- Most subsequent commands can use that saved session and saved credentials without re-running
  `tg auth` or setting environment variables.

If there is already a pending login state (for example waiting for code/password), run `tg auth`
again to continue the prompts.

## Auth/Env Troubleshooting

- `API ID must be a number`
  - Enter a numeric value for the API ID prompt.
- `API credentials not found at ...`
  - Run `tg auth` to create the credentials file.
- `Not authenticated. Run 'tg auth' first.`
  - Complete the auth flow, then rerun the command.

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
tg delete-chat --id 123456789 --through 1790000000   # NEVER against a real chat from a test or an agent
echo '{"123": 42, "-1001666847309": 89508544512}' | tg sync [--limit 1000]
echo '{"123": 0, "-1001666847309": 0}' | tg sync --reconcile-days 7
echo '{"123": 42, "-1001666847309": 0}' | tg sync --oldest-first --limit 100
echo '{"123": 42}' | tg sync --oldest-first --report-deleted
tg stream                      # NDJSON events from a running `tg serve`; never starts its own client
```

## Docs

What goes where, so this file stays small: `tests/agents_md.rs` holds it to 24 KB with no line over
400 characters and its links resolving, and `CLAUDE.md` to a link to it.

- This file: the rules every change keeps, the layout, build and test, landing. No feature
  descriptions and no history: write the current state, and leave dates, versions and tickets to
  the commit message and the ticket.
- How a feature works as built (what TDLib does, read from its source or measured): its section in
  `docs/architecture/<area>.md`, indexed in
  [docs/architecture/README.md](docs/architecture/README.md). A new feature gets a section there.
- The contracts other programs read: `README.md` (the CLI's output and the wire protocol) and
  `docs/specs/2026-05-22-serve-design.md` (the event frames). The notes behind the tests against a
  real TDLib: [docs/build-and-test.md](docs/build-and-test.md). Designs and plans: `docs/specs/`,
  `docs/plans/`.
