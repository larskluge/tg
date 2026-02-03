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

Required for runtime:
- `TG_API_ID` - Telegram API ID from my.telegram.org
- `TG_API_HASH` - Telegram API hash

Optional (alternative to CLI flags):
- `TG_PHONE` - Phone number for auth

## CLI Examples

```bash
tg auth --phone +1234567890   # Step 1: Send phone number (code sent to Telegram)
tg auth                       # Step 2: Enter verification code (and 2FA password if enabled)
tg chats [--limit 50] [--json]
tg groups [--limit 50]
tg unread
tg send "John Doe" -m "Hello!"
tg send --id 123456789 -m "Hello!"
tg send --group "Family" -m "Hi all!"
tg messages "John Doe" [--limit 20]
tg search "John"
tg mark-read "John Doe"
tg mark-unread --id 123456789
```

## Architecture

Telegram CLI client using TDLib via `tdlib-rs` with `download-tdlib` feature.

**Key modules:**
- `cli.rs` - Clap-based CLI definitions; auth flags support both CLI args and env vars (CLI takes precedence)
- `error.rs` - Custom error types using thiserror; use `TgError` variants and `Result<T>` alias
- `client.rs` - TDLib client wrapper with `TelegramClient` trait for mocking
- `output.rs` - Dual output formatting (plain text default, JSON with `--json`)
- `commands/` - One file per command

**Testing pattern:** Mock `TelegramClient` trait for unit tests. CLI parsing tests use `Cli::parse_from()`.

**Session storage:** `~/.local/share/tg/`

**TDLib types:** Functions return enums wrapping types (e.g., `tdlib_rs::enums::Chat::Chat(c)` → `tdlib_rs::types::Chat`). Use helper functions like `unwrap_chat()` in client.rs.
