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
- **Long-lived server** — `tg serve` keeps a TDLib client warm so every other `tg <cmd>` skips cold start
- **Bulk sync** — fetch new messages for multiple chats in a single session (machine use)
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

# Run the long-lived server so other commands skip cold start
tg serve
```

## Machine use

`tg serve` runs a long-lived background process that keeps one TDLib client warm and exposes it over a Unix socket. Every other `tg <cmd>` automatically routes through it when it's up — and falls back to in-process TDLib (today's behaviour) when it isn't. The server is a pure performance optimisation; nothing breaks without it.

### Running the server

```bash
tg serve                                  # foreground
podman exec -d tg tg serve                # backgrounded inside a container
```

The socket path is resolved in this order:

1. `TG_SERVE_SOCKET=/explicit/path` — use that path verbatim.
2. `TG_SERVE_SOCKET=` (set but empty) — disabled; clients always use in-process TDLib.
3. `$XDG_RUNTIME_DIR/tg.sock` if `XDG_RUNTIME_DIR` is set.
4. `$DATA_DIR/tg/serve.sock` (e.g. `~/Library/Application Support/tg/serve.sock` on macOS).

The socket is created with `0600` permissions. Concurrent connections are accepted, but TDLib calls are serialised internally — slow commands (e.g. `download`) block the channel until they complete.

**Operational notes:**

- `tg auth`, `tg auth bot`, and `tg auth status` cannot run while `tg serve` is active. Stop the server first, run auth, then start the server again.
- Bot sends (`tg send --as <bot>`) use the HTTP API and bypass the socket — they work whether the server is up or not.
- Restarting the server gives you a cold TDLib but the on-disk session survives, so re-auth is not needed.

### Wire protocol (for non-`tg`-CLI clients)

Newline-delimited JSON on the Unix socket. One request per line, one response per line, in arrival order.

Request: `{"id": "<opaque>", "cmd": "<command>", "args": { ... }}`
Response (success): `{"id": "<echoed>", "ok": true, "result": <value>}`
Response (failure): `{"id": "<echoed>", "ok": false, "error": "<message>"}`

`cmd` is one of: `whoami`, `chats`, `groups`, `unread`, `search`, `messages`, `send`, `download`, `mark_read`, `mark_unread`, `sync`. `args` field names are snake_case and match the corresponding CLI flags. The `result` shape matches each command's `--json` output today.

### One-shot bulk sync

`tg sync` is a one-shot variant of the server's `sync` command for callers that don't want to maintain a long-lived child. It reads a JSON map of `{chat_id: last_message_id}` from stdin and outputs results keyed by chat ID. Works whether or not `tg serve` is up — when the server is running, the request goes through the socket; otherwise it cold-starts TDLib.

```bash
# Stdin: map of chat ID (string) → last seen message ID (integer)
# Use 0 as the message ID to fetch all recent messages (no prior state)
echo '{"123": 42, "-1001666847309": 89508544512}' | tg sync

# Override all HWMs with a date-based cutoff (for reconciliation sweeps)
echo '{"123": 0, "-1001666847309": 0}' | tg sync --reconcile-days 7

# Limit messages per chat (default: 1000)
echo '{"123": 0}' | tg sync --limit 500
```

Output is always JSON, keyed by chat ID. Each value is an array of messages or an error object:

```json
{
  "123": [{"id": 43, "chat_id": 123, "sender": "Alice", "text": "hello", ...}],
  "-1001666847309": [],
  "999": {"error": "Chat not found"}
}
```

The HWM boundary message itself is excluded from results (it was already consumed). Exit code is 0 if all chats succeeded, 1 if any chat had an error (successful results are still in the output).

## Testing

```bash
cargo test              # Run all tests
cargo test cli::tests   # Run CLI tests only
cargo clippy            # Lint
cargo fmt               # Format
```

## License

MIT
