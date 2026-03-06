# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

`tg` reads two `TG_*` vars:

- `TG_API_ID` (required for `tg auth` when credentials are not already stored)
  - Telegram API ID from `my.telegram.org`
  - Must parse as a number (`i32`)
- `TG_API_HASH` (required for `tg auth` when credentials are not already stored)
  - Telegram API hash from `my.telegram.org`
Example setup:

```bash
export TG_API_ID=123456
export TG_API_HASH=0123456789abcdef0123456789abcdef
```

Notes:
- `tg auth` prefers `TG_API_ID`/`TG_API_HASH` when set.
- On successful `tg auth`, `tg` persists API credentials under `dirs::data_dir()/tg/credentials.json`.
- Non-auth commands (e.g. `tg groups`) read persisted credentials and do not require `TG_API_ID`/`TG_API_HASH`.

## Authentication Flow

Session data is stored in `dirs::data_dir()/tg` (typically `~/Library/Application Support/tg` on macOS and `~/.local/share/tg` on Linux).

Typical first-time auth:

```bash
TG_API_ID=123456 TG_API_HASH=0123456789abcdef0123456789abcdef tg auth --phone +1234567890
```

What happens during `tg auth`:
1. Phone number is submitted from `--phone`.
2. CLI prompts for the Telegram verification code.
3. If 2FA is enabled, CLI prompts for the password.
4. On success: `Authenticated successfully!`

Outcome:
- `tg` stores authenticated session state under `dirs::data_dir()/tg` (for example `~/Library/Application Support/tg` on macOS).
- `tg` also stores API credentials in `dirs::data_dir()/tg/credentials.json`.
- Most subsequent commands can use that saved session and saved credentials without re-running `tg auth` or setting `TG_API_ID`/`TG_API_HASH`.

If there is already a pending login state (for example waiting for code/password), run `tg auth` again to continue the prompts.

## CLI Examples

```bash
TG_API_ID=123456 TG_API_HASH=0123456789abcdef0123456789abcdef tg auth --phone +1234567890
tg chats [--limit 50] [--json]
tg groups [--limit 50]
tg unread
tg send "John Doe" -m "Hello!"
tg send --id 123456789 -m "Hello!"
tg send --group "Family" -m "Hi all!"
tg messages "John Doe" [--limit 20] [--since-utc 2026-03-01]
tg messages --chat -1001666847309 [--limit 20] [--since-utc 2026-03-01]
tg download --chat -1001666847309 --message 42 [--output-dir .] [--priority 16]
tg search "John"
tg mark-read "John Doe"
tg mark-unread --id 123456789
```

## Architecture

Telegram CLI client using TDLib via `tdlib-rs` with `download-tdlib` feature.

**Key modules:**
- `cli.rs` - Clap-based CLI definitions; auth uses `--phone` for first-time login
- `credentials.rs` - API credential loading/saving (`TG_API_ID`/`TG_API_HASH` and `credentials.json`)
- `error.rs` - Custom error types using thiserror; use `TgError` variants and `Result<T>` alias
- `client.rs` - TDLib client wrapper with `TelegramClient` trait for mocking
- `output.rs` - Dual output formatting (plain text default, JSON with `--json`)
- `commands/` - One file per command

**Testing pattern:** Mock `TelegramClient` trait for unit tests. CLI parsing tests use `Cli::parse_from()`. Internal algorithms (e.g. `collect_messages_paginated`, `collect_filtered_chats_from_source`) are extracted as free functions taking a source trait so they can be tested without TDLib.

**Session storage:** `dirs::data_dir()/tg` (typically `~/Library/Application Support/tg` on macOS and `~/.local/share/tg` on Linux), including `credentials.json`

## Auth/Env Troubleshooting

- `Environment variable TG_API_ID not set`
  - Set `TG_API_ID` before initial `tg auth`.
- `Environment variable TG_API_HASH not set`
  - Set `TG_API_HASH` before initial `tg auth`.
- `TG_API_ID must be a number`
  - Use a numeric value only when running `tg auth`.
- `API credentials not found at .../credentials.json`
  - Run `tg auth --phone <number>` with `TG_API_ID` and `TG_API_HASH` set to create the credentials file.
- `Phone number required. Run: tg auth --phone +1234567890`
  - Provide `--phone` for initial phone submission.
- `Not authenticated. Run 'tg auth' first.`
  - Complete the auth flow, then rerun the command.

**TDLib types:** Functions return enums wrapping types (e.g., `tdlib_rs::enums::Chat::Chat(c)` → `tdlib_rs::types::Chat`). Use helper functions like `unwrap_chat()` in client.rs.

**TDLib `getChatHistory` quirk:** May return fewer messages than `limit` on the first call while syncing from the server. Always use a retry+pagination loop: retry on empty responses (up to 5×), and page using the oldest returned message ID as the next `from_message_id`.

**Clap negative IDs:** Telegram supergroup IDs are negative (e.g. `-1001666847309`). Any `--id` arg that accepts `i64` needs `#[arg(long, allow_hyphen_values = true)]` or clap will treat the leading `-` as a flag.
