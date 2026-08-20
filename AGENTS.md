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
```

## Architecture

Telegram CLI client using TDLib via `tdlib-rs` with `download-tdlib` feature.

**Key modules:**
- `cli.rs` - Clap-based CLI definitions
- `credentials.rs` - API credential loading/saving (`TG_API_ID`/`TG_API_HASH` and `credentials.json`)
- `error.rs` - Custom error types using thiserror; use `TgError` variants and `Result<T>` alias
- `client.rs` - TDLib client wrapper with `TelegramClient` trait for mocking
- `output.rs` - Dual output formatting (plain text default, JSON with `--json`)
- `commands/` - One file per command (`sync.rs` handles bulk message sync for machine consumers)

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

**TDLib `getChatMessageByDate` direction:** It returns the last message sent **no later than** the given date — the returned message's date is always `<= date` — and a **404** when the chat has no such message. It does not find the first message *after* a date. To turn a `--since-utc` cutoff into a fetch boundary, probe at `cutoff - 1` and use the returned message's `id + 1` as an exclusive lower bound (`boundary_probe_date` / `boundary_from_probe` in `client.rs`). Reading it as "at or after the date" makes the lookup silently never match.

**TDLib HTML parse mode is not HTML:** `textParseModeHTML` accepts only Telegram's tag
whitelist — `b`/`strong`, `i`/`em`, `u`/`ins`, `s`/`strike`/`del`, `a href`, `code`, `pre`
(+ `code class="language-x"`), `blockquote` (optionally `expandable`), `tg-spoiler`,
`tg-emoji`. There are no headings, lists, `hr`, `p` or `br`. TDLib **errors** on any tag
outside the set (it does not strip it), so `<h1>x</h1>` fails the send rather than degrading.
Tag names are case-insensitive. Only `&`, `<`, `>` need escaping, and only `lt`/`gt`/`amp`/
`quot` decode. Related facts, all measured against TDLib 1.8.61:

- `parse_text_entities` returns `enums::FormattedText`, so it needs `unwrap_formatted_text()`
  before it can go into `InputMessageText` (see **TDLib types** above). It also takes an
  owned `String`.
- TDLib does **not** auto-detect bold/italic/code server-side — its entity auto-detection
  covers only link-ish entities. That is why `entities: vec![]` delivered literal asterisks
  for years, and why a parse mode has to be requested explicitly.
- Entity offsets are UTF-16 and TDLib computes them. Never hand-roll them: always call
  `parse_text_entities`. Verified exact across emoji, umlauts, combining marks, ZWJ sequences
  and regional indicators (🚀 = 2 units, 👩‍💻 = 5, 🇩🇪 = 2), and the tdlib-rs hop is lossless
  because `types::FormattedText` and `enums::TextEntityType` both round-trip through serde.
- `MarkdownV2` is parser `version: 2`. Versions 0 and 1 are the legacy, laxer "Markdown"
  mode: picking one by mistake parses the body under the wrong rules with no error anywhere
  (pinned by `tdlib_parse_mode_markdown_is_version_2`).
- `parseTextEntities` is a TDLib **static request** — it answers on a client that has never
  called `setTdlibParameters`, needs no authorization, makes no network call, and costs
  ~17 µs. Calling it before `create_private_chat` therefore leaves zero residue when the
  markup is bad: no message, no draft, no opened chat.
- MarkdownV2's five pairable reserved characters (`` _ * ~ | ` ``) **corrupt silently**:
  `path /usr/local/bin/x_y_z` parses `Ok` to `path /usr/local/bin/xyz` with an italic entity.
  The other thirteen hard-error. Prefer `HTML`.

**Serve request strictness:** `SendRequest` is the only serve request struct carrying
`#[serde(deny_unknown_fields)]`. The others are deliberately open: `WhoamiRequest{}` backs
the container's `HealthCmd` (`tg whoami`, `HealthStartupTimeout=2m`), so tightening it risks
the health gate for no benefit, and the remaining structs have caller sets that were never
audited. `dispatch_other_commands_still_ignore_unknown_args` pins the decision so a later
blanket change has to be deliberate.

**Clap negative IDs:** Telegram supergroup IDs are negative (e.g. `-1001666847309`). Any `--id` arg that accepts `i64` needs `#[arg(long, allow_hyphen_values = true)]` or clap will treat the leading `-` as a flag.
