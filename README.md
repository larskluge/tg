# tg

A modern CLI tool for interacting with Telegram, built in Rust using [TDLib](https://core.telegram.org/tdlib).

## Features

- **Authentication** — interactive login with phone number, verification code, and optional 2FA
- **Bot support** — authenticate bots and send messages as a bot via the Telegram Bot HTTP API
- **Chats & Groups** — list direct message chats, group chats, or unread conversations
- **Messages** — read message history with date filtering (`--since-utc`)
- **Send** — send messages to contacts or groups by name, @username, or chat ID
- **Search** — find contacts by name
- **Download** — download media attachments from messages
- **Mark read/unread** — manage read state of chats
- **JSON output** — pass `--json` to any command for machine-readable output

## Requirements

- Rust 2024 edition (1.85+)
- A Telegram account
- API credentials from [my.telegram.org](https://my.telegram.org)

TDLib is downloaded automatically during build via the `tdlib-rs` crate's `download-tdlib` feature.

## Building

```bash
cargo build                # Debug build
cargo build --release      # Release build

make release               # Release build + copy TDLib library
make install               # Install symlink to ~/bin (BIN_DIR=... to override)
```

## Authentication

Run `tg auth` to start an interactive login:

```bash
tg auth
```

You will be prompted for:

1. **API ID** and **API hash** (from [my.telegram.org](https://my.telegram.org))
2. **Phone number** (E.164 format, e.g. `+1234567890`)
3. **Verification code** sent to your Telegram app
4. **2FA password** (if enabled)

Credentials and session data are stored in your OS data directory (`~/Library/Application Support/tg` on macOS, `~/.local/share/tg` on Linux). Subsequent commands use the saved session automatically.

You can also provide credentials via environment variables:

```bash
export TG_API_ID=123456
export TG_API_HASH=0123456789abcdef0123456789abcdef
export TG_PHONE=+1234567890
tg auth
```

### Bot authentication

Authenticate a bot using its token from [@BotFather](https://t.me/BotFather):

```bash
tg auth bot
tg auth bot --token 123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11
```

The token can also be set via `TG_BOT_TOKEN`. Bot credentials (username, ID, token) are stored alongside your user credentials.

## Usage

```bash
# List chats
tg chats [--limit 50] [--json]
tg groups [--limit 50]
tg unread

# Read messages
tg messages "John Doe" [--limit 20] [--since-utc 2026-03-01]
tg messages --chat -1001666847309 [--limit 20]

# Send messages
tg send "John Doe" -m "Hello!"
tg send --id 123456789 -m "Hello!"
tg send --to @username -m "Hello!"
tg send --group "Family" -m "Hi all!"

# Send as a bot (plain text only, no Markdown/HTML formatting)
tg send --as @mybot --to @someone -m "Hello from bot!"
tg send --as @mybot --to 123456789 -m "Hello!"

# Download media
tg download --chat -1001666847309 --message 42 [--output-dir .] [--priority 16]

# Search contacts
tg search "John"

# Manage read state
tg mark-read "John Doe"
tg mark-unread --id 123456789
```

## Testing

```bash
cargo test              # Run all tests
cargo test cli::tests   # Run CLI tests only
cargo clippy            # Lint
cargo fmt               # Format
```

## License

MIT
