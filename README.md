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

# Formatted messages (see "Message formatting" below)
tg send --to @username --parse-mode HTML -m "<b>bold</b> and <code>code</code>"
printf '<b>bold</b>\n<i>italic</i>' | tg send --to @username --parse-mode HTML

# Send as a bot
tg send --as @mybot --to @someone -m "Hello from bot!"
tg send --as @mybot --to 123456789 -m "Hello!"
tg send --as @mybot --to @someone --parse-mode HTML -m "<b>Hello</b>"

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

### Message formatting

`--parse-mode` (socket arg `parse_mode`) accepts exactly `HTML` and `MarkdownV2`,
case-sensitive. Absent means plain text — byte-for-byte the behaviour `tg` has always had.
An explicit JSON `null` on the socket means the same as absent, because that is what an
unset optional serialises to in Go, in Python and in `tg`'s own CLI proxy. Any other value,
including `html` or an empty string, is refused with

```
invalid parse_mode '<value>'. Expected `HTML` or `MarkdownV2`
```

and **nothing is sent** — `tg` never quietly downgrades a formatted body to plain text.

TDLib parses the markup **locally** (`parseTextEntities` is a static request: no Telegram
round trip, nothing to rate-limit or retry), so malformed markup comes back as TDLib's own
error text and nothing is sent then either. Errors are prefixed `parse_mode <MODE>: ` so a
caller can tell a permanent markup fault from a transient send failure; retrying the same
body will fail identically. Only some errors name a byte offset — the unsupported-tag and
unterminated-entity classes do, the "character is reserved" class does not — so do not build
a repair loop that depends on finding one.

**Use `HTML`.** It is the safer of the two by a wide margin, and it is the recommended default.

`HTML` is not general HTML — it is Telegram's tiny tag whitelist:

`b`/`strong`, `i`/`em`, `u`/`ins`, `s`/`strike`/`del`, `a href`, `code`,
`pre` (and `pre` + `code class="language-rust"`), `blockquote` (optionally `expandable`),
`tg-spoiler`, `tg-emoji`.

There are no headings, lists, `hr`, `p` or `br` tags: `<h1>`, `<p>`, `<br>` and `<span>` are
errors, not ignored markup. Use `\n` for line breaks. Tag names are case-insensitive
(`<B>` works).

**Escape `&`, `<` and `>` as `&amp;`, `&lt;`, `&gt;` on every body before setting
`parse_mode=HTML` — all three, unconditionally.** Escaping selectively is the trap: a bare
`<` in prose (`a < b`) is a loud error, but two other paths are silent, and both are shapes
an agent writes often:

- **A matched pair of whitelisted tags anywhere in the body becomes formatting.**
  `wrap it in <b>...</b> tags` is delivered as `wrap it in ... tags` with "..." bolded, and
  `the <code>--parse-mode</code> flag` loses its tags the same way. `ok:true`, no error.
- **Entity decoding runs exactly once**, so text that was already escaped upstream is
  un-escaped: `type &lt;b&gt;` arrives as `type <b>`, and `&amp;amp;` arrives as `&amp;`.

Which entities decode is narrower than it looks, and is *not* a reason to skip escaping `&`:
only four **named** entities decode (`lt`, `gt`, `amp`, `quot`), so `&nbsp;`, `&apos;` and
`&copy;` stay literal — but **every numeric character reference decodes**, decimal and hex
alike (`&#8364;` → `€`, `&#x41;` → `A`, `&#x1F600;` → 😀). An unmatched surrogate escape such
as `&#xD800;` is a hard error, so a body merely *discussing* one fails the send. A bare `&`
not followed by an entity (`AT&T`) does pass through unchanged.

**`MarkdownV2` is dangerous for text that was not written as MarkdownV2**, which is why it
is not the recommendation. It reserves eighteen characters —
``_ * [ ] ( ) ~ ` > # + - = | { } . !`` — plus the backslash, which the character list
conventionally omits because it is the escape character itself. Two failure modes:

- **Loud:** a reserved character on its own hard-errors — including the pairable ones, whose
  error is about the unterminated entity rather than the character:
  `hello. world!` → ``Character '.' is reserved and must be escaped``,
  `5 * 3 = 15` → `'=' is reserved`, `cost is $5 (approx)` → `'(' is reserved`,
  `a | b` → `'|' is reserved`, `x_y` → ``Can't find end of Italic entity``.
- **Silent:** the pairable ones **corrupt the message with no error at all** when the body
  happens to contain a matching pair — `` _ `` (italic), `*` (bold), `~` (strikethrough),
  `` ` `` (code), `__` (underline), `||` (spoiler) and `[…]` (deleted outright without a
  following `(url)`, a link with one). So does a `>` at the **start of a line** (blockquote —
  the one reserved character that is silent even alone; mid-line it errors), and a backslash
  anywhere, which is eaten always, even singly.

  ```
  in:  path /usr/local/bin/x_y_z      out: path /usr/local/bin/xyz   ← "y" italicised
  in:  see [attachment] for details   out: see attachment for details
  in:  backup is at C:\Data\2026      out: backup is at C:Data2026
  in:  > he said the deal is off      out: " he said the deal is off" ← blockquote
  ```

  All four are `ok:true` with characters deleted. Paths, `snake_case` identifiers, Windows
  paths, bracketed asides and quoted lines are the most common shapes in an agent-written
  message, so this is not a corner case. Under `MarkdownV2` the **caller** carries 100% of
  the escaping burden: `tg` passes the body to TDLib verbatim and computes no offsets and
  escapes nothing on the caller's behalf.

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

`send` rejects unknown `args` keys rather than ignoring them:

```json
{"id": "1", "ok": false, "error": "invalid args: unknown field `x`, expected one of `message`, `name`, `id`, `to`, `group`, `parse_mode`"}
```

The other commands still ignore unknown keys. For a recipient or identity field, a silent drop
means a message delivered to the wrong place with `ok: true`, which is worse than a refusal the
caller can retry.

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

### Bot markers

Every JSON surface carries Telegram's own bot flag, so consumers never have to guess from a display name (a person can be surnamed "Talbot"; a bot can be named anything):

- `sender_is_bot` on each message (`tg messages --json`, `tg sync`)
- `is_bot` on each chat (`tg chats --json`, `tg groups --json`, `tg unread --json`), contact (`tg search --json`) and user (`tg whoami --json`)

The value is `true` or `false` when `tg` could determine it, and `null` when it could not. `null` means unknown, never "not a bot": the sender's user object was unreadable (the same case that leaves `sender` as `"Unknown"`), the chat has no single user counterpart (groups and channels), or the payload came from a `tg` older than 0.4.4. A message sent by a channel or group rather than by a user account is `false` — a chat sender carries no bot marker either way. Deleted and inaccessible accounts also report `false`: Telegram reports them as `userTypeDeleted`/`userTypeUnknown` and no longer says whether they were bots.

## Testing

```bash
cargo test              # Run all tests
cargo test cli::tests   # Run CLI tests only
cargo clippy            # Lint
cargo fmt               # Format
```

## License

MIT
