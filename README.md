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
- **Reactions** — every listed message carries its emoji reactions; `tg react` adds one or takes it back, and `tg message` reads one message again
- **Mark read/unread** — manage read state of chats
- **Long-lived server** — `tg serve` keeps a TDLib client warm so every other `tg <cmd>` skips cold start
- **Live events** — `tg stream` prints new, edited and deleted messages from `tg serve` as they happen (machine use)
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

# Read one message again, wherever it lies in the chat, reactions included
tg message --chat -1001666847309 --message 89508544512 [--json]

# React to a message, or take the reaction back
tg react --chat -1001666847309 --message 89508544512 👍
tg react --chat -1001666847309 --message 89508544512 👍 --remove
tg react --chat -1001666847309 --message 89508544512 --remove    # whatever reaction you have there

# Download media
tg download --chat -1001666847309 --message 42 [--output-dir .] [--priority 16]

# Search contacts
tg search "John"

# Manage read state
tg mark-read "John Doe"
tg mark-unread --id 123456789

# Run the long-lived server so other commands skip cold start
tg serve

# Follow message events from the running server, one JSON line each
tg stream
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

One failure carries both: a media `send` that delivered some elements and then
could not finish answers `ok: false` with the error **and** a `result` holding the
per-element record — see [`send` with attachments](#send-with-attachments). `ok`
keeps its meaning (the request did not do what was asked), so a caller that reads
only `ok` is never told a failure succeeded; what it gains is being able to learn
which elements arrived instead of assuming none did. Every other failure has no
`result`.

`cmd` is one of: `whoami`, `chats`, `groups`, `unread`, `search`, `messages`, `message`, `send`, `react`, `download`, `mark_read`, `mark_unread`, `sync`, `subscribe`. `args` field names are snake_case and match the corresponding CLI flags. The `result` shape matches each command's `--json` output today. `subscribe` is different: it turns the connection into an event stream — see [Live events](#live-events-subscribe-and-tg-stream-since-0110).

`send` rejects unknown `args` keys rather than ignoring them:

```json
{"id": "1", "ok": false, "error": "invalid args: unknown field `x`, expected one of `message`, `name`, `id`, `to`, `group`, `parse_mode`, `files`, `reply_to`"}
```

So does [`react`](#react-since-0120). The other commands still ignore unknown keys. For a
recipient or identity field, a silent drop means a message delivered to the wrong place with
`ok: true`, which is worse than a refusal the caller can retry.

#### `send` with attachments

`args.files` sends 1-10 local files as **one** Telegram message, with `args.message` as the
caption. Omit the key entirely (or pass `null`) for a text-only send — that path is byte-identical
to the pre-`files` contract, so a caller that never attaches anything needs no change.

```json
{"id": "1", "cmd": "send", "args": {
  "to": "@someone",
  "message": "Q3 numbers attached",
  "parse_mode": "HTML",
  "files": [
    {"path": "/outbox/42/0/Q3 report.pdf", "kind": "file"},
    {"path": "/outbox/42/1/chart.png", "kind": "photo", "width": 1024, "height": 768}
  ]
}}
```

Per entry:

| field | meaning |
|---|---|
| `path` | **required**, absolute, must exist and be readable *by the daemon* |
| `kind` | `"photo"` or `"file"`; absent means `"file"` |
| `width`/`height` | photo pixel dimensions; `0` or absent lets Telegram work them out. Only read for `"photo"` |

`files[]` is as closed as `send` itself — an unknown key is refused:

```json
{"id": "1", "ok": false, "error": "invalid args: unknown field `mime`, expected one of `path`, `kind`, `width`, `height`"}
```

**The recipient sees the path's basename.** No TDLib input type carries a filename or a MIME
type, so the *only* way to control the delivered name is the name on disk; `tg` renames nothing.
Materialise each file under the name it should arrive as (a per-file directory is the simplest way
to keep two files with the same basename apart, as in the example above).

Refused in-band, with nothing sent, before any Telegram lookup:

- `files` present but empty (omit the key instead)
- more than 10 entries (`a Telegram album carries at most 10`)
- an unknown `kind`
- `"photo"` and `"file"` mixed in one request — TDLib groups only same-typed contents, so there is
  no single message to send; split it into two
- a relative path, a missing file, a directory, or a file the daemon cannot open
- a `width`/`height` on a `"file"` entry, or a negative dimension

**Confirmation differs from a text send, deliberately.** A local file is uploaded *after* the send
call returns, so `send` with `files` waits up to **300s** for Telegram to confirm every element and
answers `ok: false` if it does not — it never returns a temporary message id as a success. A text
send's 10s behaviour is unchanged (it still answers `ok: true` with the local id on timeout). On
success, `result.message_id` is the real id of the first element, the one carrying the caption.

##### Per-element outcomes (since 0.6.0)

**An album is N independent Telegram messages**, each confirmed separately, so "the send failed"
is not a fact about the message: elements Telegram already confirmed are in the recipient's chat
and nothing takes them back. A media `send` therefore reports what happened **per element**, on
success and on failure alike, under `result.elements`:

| list | meaning | what to do |
|---|---|---|
| `delivered` | `[{"index": N, "message_id": M}]`, ascending by index — Telegram confirmed these | record them; **never resend** |
| `failed` | `[{"index": N, "error": "..."}]` — Telegram reported these as failed | nothing was delivered, so these and only these are safe to resend |
| `unconfirmed` | `[N, ...]` — bare indices whose outcome never arrived | TDLib may still be uploading; **do not resend blindly**, check the chat |

`index` is the element's position in the request's `files` array. The three lists are disjoint and
together cover every element of the request, so "not in `delivered`" is a complete answer to what
still has to be sent. An empty list is omitted rather than sent as `[]`.

Full success — every element confirmed:

```json
{"id": "1", "ok": true, "result": {
  "message_id": 900,
  "chat_id": 42,
  "elements": {"delivered": [{"index": 0, "message_id": 900}, {"index": 1, "message_id": 901}]}
}}
```

Partial delivery — `ok: false`, with the record of what arrived:

```json
{"id": "1", "ok": false,
 "error": "send: 1 of 2 file(s) were not delivered; ALREADY DELIVERED, do not resend: files[0] ('a.jpg') as message 900; not delivered, safe to resend: files[1] ('b.jpg'): PHOTO_INVALID_DIMENSIONS",
 "result": {
   "chat_id": 42,
   "elements": {
     "delivered": [{"index": 0, "message_id": 900}],
     "failed": [{"index": 1, "error": "PHOTO_INVALID_DIMENSIONS"}]
   }
 }}
```

The failure `result` has **no `message_id`**: there is no single id for a send that did not
complete, and inventing one would report a delivery Telegram never confirmed. Read
`result.elements`.

An expired 300s deadline produces the same shape with `unconfirmed` in place of `failed` — the
elements Telegram confirmed are still reported, and the rest are still uploading rather than
known-lost:

```json
{"id": "1", "ok": false,
 "error": "send: 1 of 2 file(s) were still uploading after 300s; ALREADY DELIVERED, do not resend: files[0] ('a.jpg') as message 900; still unconfirmed and may yet be delivered — check the chat before resending: files[1] ('b.jpg')",
 "result": {
   "chat_id": 42,
   "elements": {"delivered": [{"index": 0, "message_id": 900}], "unconfirmed": [1]}
 }}
```

**What a caller must branch on:** `ok` first (the request did not do what was asked), then
`result.elements.delivered` for the indices already in the chat — present on a failure too. A
retry sends a fresh `send` with only the elements left over; a one-element `files` array is an
ordinary single-file send, so resending one remaining file needs nothing special.

A failure carries no `result` at all when nothing reached Telegram — a refused request (every
bullet above), an unknown recipient, or a `sendMessageAlbum` that errored outright. In that case
nothing was delivered and nothing is queued.

A text send's result is unchanged and carries no `elements` key, so its presence also tells a
caller "this was a media send".

A daemon that predates attachments refuses the key loudly — "unknown field \`files\`" — and
delivers nothing, so no capability probe is needed.

#### `send` as a reply (since 0.7.0)

`args.reply_to` sends the message — text, one file or an album — as a **native Telegram reply** to
a message already in the destination chat:

```json
{"id": "1", "cmd": "send", "args": {"message": "Thanks, fixed.", "id": -1009876543210, "reply_to": 962592768}}
-> {"id": "1", "ok": true, "result": {"message_id": 963641344, "chat_id": -1009876543210, "reply_to_message_id": 962592768}}
```

- `reply_to` is a **TDLib message id**: `server_id << 20`, exactly what `tg messages` and `tg sync`
  print as `id`. It is never the bare server id a `t.me/c/…/918` link shows; a value that is not
  a positive multiple of 2^20 is refused before the recipient is resolved (`invalid reply_to …`).
- The target must be in **the chat being sent to**. A TDLib message id only means something within
  its chat, and the same number can name an unrelated message elsewhere, so `send` proves the
  target first — `getMessage` in that chat, then `messageProperties.can_be_replied` — and refuses
  with `reply_to message … is not accessible in chat …` (or `cannot be replied to`) having sent
  nothing. The check exists because TDLib does not refuse a reply it cannot honour: it sends the
  message anyway, unthreaded. It is bounded at 10s (a target TDLib has not cached is fetched
  over the network) and refuses on expiry, again having sent nothing.
- `result.reply_to_message_id` is present only when TDLib **confirmed** it attached that reply —
  read from the message the server accepted: on a media send, from the first element's
  confirmation once its upload lands, never from the queued copy. A send that asked for a reply
  and came back without the key may have been delivered as an ordinary message.
- A client can prove a daemon carries `reply_to` without sending anything: a recipient-less
  request with `"reply_to": 1` is answered `invalid reply_to 1: …` by 0.7.0 and later, but
  `unknown field` (0.4.6-0.6.x) or the missing-recipient error (older, which drop the field).
- Absent `reply_to` is an ordinary send, byte-identical to before. The CLI has no flag for it;
  like `files`, it is a socket-protocol feature.

A daemon that predates replies refuses the key loudly — "unknown field \`reply_to\`" — and
delivers nothing.

#### `react` (since 0.12.0)

`react` puts the account's emoji reaction on one message, or takes it back:

```json
{"id": "1", "cmd": "react", "args": {"id": -1009876543210, "message_id": 962592768, "emoji": "👍"}}
-> {"id": "1", "ok": true, "result": {"chat_id": -1009876543210, "message_id": 962592768, "emoji": "👍", "chosen": true}}

{"id": "2", "cmd": "react", "args": {"id": -1009876543210, "message_id": 962592768, "emoji": "👍", "remove": true}}
-> {"id": "2", "ok": true, "result": {"chat_id": -1009876543210, "message_id": 962592768, "emoji": "👍", "chosen": false}}
```

| arg | | |
|---|---|---|
| `id` | i64, required | the chat |
| `message_id` | i64, required | the message's TDLib id in that chat: `server_id << 20`, as `tg messages` and `tg sync` print `id` |
| `emoji` | string | the emoji. Required to react; with `remove` it may be `""` or absent, which takes back whatever reaction the account has on the message |
| `remove` | bool, default `false` | take the reaction back instead of adding it |

- **Closed like `send`.** An unknown key is refused (`invalid args: unknown field …`), and nothing
  goes out.
- **The target is proved first.** `message_id` must be a positive multiple of 2^20 (`invalid
  message_id …` otherwise, before TDLib is asked anything), and the message must be in that chat
  (`react: message … is not accessible in chat …: Not Found`, the last words being TDLib's).
- **Either spelling of an emoji is taken.** Telegram's reactions carry no variation selector: its
  heart is the bare `U+2764`, a keyboard's is `U+2764 U+FE0F`, and TDLib refuses a reaction that is
  not byte for byte one Telegram offers. So `react` reads the emoji Telegram offers for that
  message (`getMessageAvailableReactions`) and sends the one that equals `emoji` once variation
  selectors are set aside. `result.emoji` is the spelling Telegram holds. An emoji Telegram offers
  there in no spelling is sent as given, so the refusal is Telegram's own.
- **Success is read back.** After Telegram answers, the message is read again: `ok: true` always
  means it shows what was asked for, `chosen: true` after reacting and `chosen: false` after
  taking back. A read-back that disagrees is `ok: false`.
- **One reaction per message** for an account without Telegram Premium: another emoji replaces the
  one the account had. Taking back a reaction that is not there changes nothing and answers
  `chosen: false`. Repeating a request is safe either way.
- **Telegram's reason is passed through**, and no error names the emoji:
  `react: Telegram refused the reaction to message 962592768 in chat -1009876543210: The reaction isn't available for the message`
  (the chat does not allow that emoji, or the string is no reaction).
- It is not sent big, and it counts among the account's recent reactions, as one picked in a
  Telegram app does. Custom-emoji and paid reactions cannot be sent.
- Telegram's answer is awaited for 10 s. Past that the request fails and says the change may still
  take effect: TDLib shows a reaction on its own copy of the message before Telegram has answered.
- A client can learn whether a daemon knows `react` without reacting to anything: `"args": {}` is
  answered ``invalid args: missing field `id` `` by 0.12.0 and later, and `unknown command: react`
  before.

The CLI is `tg react --chat <id> --message <id> <emoji> [--remove]`.

### Live events: `subscribe` and `tg stream` (since 0.11.0)

A `subscribe` request turns its connection into a stream of TDLib update events. After the usual
reply, every line is an event frame, `{"event": <name>, "data": {...}}`:

```json
{"id": "1", "cmd": "subscribe", "args": {}}
{"id": "1", "ok": true, "result": {"subscribed": true}}
{"event": "new_message", "data": {"chat_id": -1001666847309, "message_id": 89508544512}}
{"event": "chat_last_message", "data": {"chat_id": -1001666847309, "message_id": 89508544512}}
{"event": "heartbeat", "data": {}}
```

| event | data | when |
|---|---|---|
| `new_message` | `{"chat_id": i64, "message_id": i64}` | TDLib `updateNewMessage` |
| `message_edited` | `{"chat_id": i64, "message_id": i64}` | `updateMessageContent` or `updateMessageEdited` — one edit usually sends both |
| `messages_deleted` | `{"chat_id": i64, "message_ids": [i64]}` | `updateDeleteMessages`, only when permanent and not merely a cache eviction |
| `chat_last_message` | `{"chat_id": i64, "message_id": i64 \| null}` | `updateChatLastMessage`; `null` when TDLib no longer knows the last message, and while it does not, new messages can arrive without `new_message` |
| `message_reactions` | `{"chat_id": i64, "message_id": i64}` | `updateMessageInteractionInfo` (since 0.12.0): the message's reactions may have changed |
| `lagged` | `{"skipped": u64}` | this subscriber fell more than 1024 updates behind and lost `skipped` of them |
| `heartbeat` | `{}` | every 30 s |

An event is a **doorbell, not a record**: it names a chat and a message and carries no content.
Fetch what changed with `sync`, or with [`message`](#one-message-message-since-0120) when it lies
below your cursor. A missed event costs latency, never data — after `lagged`, and after
connecting, re-sync every chat you follow.

- A subscription never waits on the server's TDLib lock, so a long `download` or `sync` does not
  delay events.
- It ends when you close the connection — a half-close too, so keep your write side open.
  Anything you send after `subscribe` is ignored.
- The heartbeat's job is to find a subscriber that vanished: its failed write ends the
  subscription.
- `tg serve` ends every subscription when it shuts down.
- The first `chats`, `groups` or `unread` request after `tg serve` starts rings one
  `chat_last_message` per chat it loads (hundreds, once per server session).
- Supergroups and channels may not ring at all: TDLib delivers their updates "only for opened
  chats", and `tg` opens none. Poll them as before.
- `message_reactions` is the only sign of a reaction: Telegram does not move a message's edit date
  for one. It rings for the message's whole interaction info, so also for a channel post's view and
  forward counters and a discussion thread's reply count: read the message and compare its
  `reactions` with the ones you hold. Which reactions ring at all is under
  [Reactions](#reactions-since-0120).

`tg stream` is the CLI for it: it subscribes and prints each event line to stdout, flushed —
never the ack. It needs a running `tg serve` and never starts its own TDLib client, because that
would be a second client on the database serve holds.

```bash
tg stream
# {"event":"chat_last_message","data":{"chat_id":123456789,"message_id":45088768}}
# {"event":"heartbeat","data":{}}
```

| exit | when |
|---|---|
| `1` | `tg serve` is not reachable, refuses `subscribe`, or closes the stream |
| `0` | stdout was closed — noticed at the next line, at the latest the heartbeat |
| `0` | stdin reached EOF |

**Keep stdin open for as long as you want events** (`tg stream < /dev/null` exits at once). Under
`podman exec -i`, stdin's EOF is the only sign that the caller has gone: podman closes the exec'd
process's stdin when its client disconnects, but conmon keeps accepting its stdout, so without
the stdin rule an orphaned `tg stream` would run forever.

A `tg serve` older than 0.11.0 answers `unknown command: subscribe`; an older `tg` has no
`stream` subcommand at all.

### One message: `message` (since 0.12.0)

`message` reads one message by its chat and its id, as the same object `messages` and `sync` list:

```bash
tg message --chat -1009876543210 --message 962592768 --json
# {"id": 962592768, "chat_id": -1009876543210, "sender_id": 5550101, "sender": "Giulia Ferraro", "text": "Lunch?", ..., "reactions": [...]}
```

```json
{"id": "1", "cmd": "message", "args": {"chat": -1009876543210, "message": 962592768}}
-> {"id": "1", "ok": true, "result": {"id": 962592768, "chat_id": -1009876543210, ...}}
```

It is the read a `message_reactions` event asks for. `sync` returns what is above a cursor or
inside a reconcile window, and a reaction lands on a message far below either. `sync
--oldest-first --limit 1` with a cursor one server id lower (`id - 2^20`) comes close and is not
the same: it loads a window of a hundred messages to return one, and when the message is gone it
answers with the next one instead of saying so.

- The answer is one object, not an array. `--json` prints it; without the flag it is one line of
  text.
- A message TDLib does not hold is fetched from Telegram, for at most 10 s.
- An id that names nothing in that chat is an error (`message … is not accessible in chat …: Not
  Found`, the last words being TDLib's), exit code 1, never another message.
- A `tg serve` older than 0.12.0 answers `unknown command: message`.

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

# Oldest-first: the OLDEST 100 messages above each HWM, ascending
echo '{"123": 42, "-1001666847309": 0}' | tg sync --oldest-first --limit 100
```

Output is always JSON, keyed by chat ID. Each value is an array of messages or an error object (or, with `--report-deleted`, the deleted-chat object below):

```json
{
  "123": [{"id": 43, "chat_id": 123, "sender": "Alice", "text": "hello", ...}],
  "-1001666847309": [],
  "999": {"error": "Chat not found"}
}
```

The HWM boundary message itself is excluded from results (it was already consumed). Exit code is 0 if all chats succeeded, 1 if any chat had an error (successful results are still in the output).

#### Which messages: `--oldest-first`

By default each chat's array holds the **newest** `--limit` messages above the HWM, newest
first. With more than `--limit` messages above the HWM, the older ones are not returned at
all — so a caller that advances its HWM to the highest id it received skips them for good
(a 2-hour outage with 250 new messages loses 150; a chat synced from HWM `0` only ever gets
its newest `--limit`).

`--oldest-first` (socket: `"oldest_first": true`) returns the **oldest** `--limit` messages
with id > HWM instead, in **ascending** id order. HWM `0` starts at the chat's first message.
Advancing the HWM to the last id returned and asking again pages through any gap without
losing a message; an empty array means the chat is caught up. Memory is bounded by `--limit`
per chat — it steps forward through TDLib's history rather than loading the gap, unlike
`tg messages --oldest-first`, which reads the whole window.

```bash
echo '{"-1001666847309": 89508544512}' | tg sync --oldest-first --limit 100
# → {"-1001666847309": [{"id": 89509593088, ...}, {"id": 89510641664, ...}, ...]}  ascending
```

```json
{"id": "1", "cmd": "sync", "args": {"hwm": {"-1001666847309": 89508544512}, "limit": 100, "oldest_first": true}}
```

`--oldest-first` cannot be combined with `--reconcile-days` (a reconcile sweep is
newest-first by design): the CLI rejects the pair, and the socket answers `ok: false`.
`sync` does not reject unknown `args` keys, so a daemon older than 0.9.0 ignores
`oldest_first` and answers newest-first — check `tg --version` before relying on it.

#### Deleted chats: `--report-deleted`

An empty array cannot tell a quiet chat from one the account has deleted: both have nothing
new to return. With `--report-deleted` (socket: `"report_deleted": true`, since 0.10.0) a
**private** chat whose fetch came back empty is also checked, and a deleted one answers an
object instead of `[]`:

```bash
echo '{"123": 42, "456": 42}' | tg sync --oldest-first --report-deleted
# → {"123": {"chat_deleted": true}, "456": []}
```

`{"chat_deleted": true}` means the chat now holds no messages at all, so every message a
caller stored from it is deleted too, however old. It is reported only when all of these hold
— the state Telegram leaves a private chat in once you delete it:

- the chat is a private chat (a group you left is never reported);
- it belongs to no chat list (Main, Archive or any folder);
- it has no last message, and reading its history from the end comes back empty;
- the first two still hold after that read.

An error anywhere is reported as `{"error": ...}`, never as a deletion. A chat that receives
a new message after being deleted is back in a list, and reads as an ordinary chat again.
Without the flag the output is exactly as before. It goes with every mode.

### Bot markers

Every JSON surface carries Telegram's own bot flag, so consumers never have to guess from a display name (a person can be surnamed "Talbot"; a bot can be named anything):

- `sender_is_bot` on each message (`tg messages --json`, `tg sync`)
- `is_bot` on each chat (`tg chats --json`, `tg groups --json`, `tg unread --json`), contact (`tg search --json`) and user (`tg whoami --json`)

The value is `true` or `false` when `tg` could determine it, and `null` when it could not. `null` means unknown, never "not a bot": the sender's user object was unreadable (the same case that leaves `sender` as `"Unknown"`), the chat has no single user counterpart (groups and channels), or the payload came from a `tg` older than 0.4.4. A message sent by a channel or group rather than by a user account is `false` — a chat sender carries no bot marker either way. Deleted and inaccessible accounts also report `false`: Telegram reports them as `userTypeDeleted`/`userTypeUnknown` and no longer says whether they were bots.

### Reply targets (since 0.8.0)

Each message from `tg messages --json`, `tg sync` and the server's `messages`/`sync` carries
`reply_to_message_id` when it is a native reply to another message **in the same chat**:

```json
{"id": 963641344, "chat_id": -1009876543210, "reply_to_message_id": 962592768, ...}
```

The id is a TDLib message id in the message's own `chat_id`, so `(chat_id, reply_to_message_id)`
names the replied message. The key is **absent** for a message that is not a reply, and also for a
reply that cannot be named that way:

- a reply to a message in **another chat** (TDLib's `messageReplyToMessage.chat_id` differs — the
  Replies chat, a quote from elsewhere). The bare id would name an unrelated message of this chat
  to any consumer that keys messages on `(chat_id, id)`, so it is dropped rather than reported.
- a reply into an **unknown chat** (TDLib reports both ids as `0`).
- a reply to a **story** (`messageReplyToStory`): a story is not a message.

A payload from a `tg` older than 0.8.0 never carries the key, reply or not.

### Reactions (since 0.12.0)

Each message from `tg messages --json`, `tg message --json`, `tg sync` and the server's
`messages`/`message`/`sync` carries `reactions` when it has any, one entry per emoji in
Telegram's order:

```json
{"id": 962592768, "chat_id": -1009876543210, ..., "reactions": [
  {"emoji": "👍", "count": 3, "chosen": true,
   "recent_senders": [{"id": 5550101, "name": "Giulia Ferraro"}, {"id": 5550102, "name": "Paolo Conti"}]},
  {"emoji": "❤", "count": 12, "chosen": false, "recent_senders": []}
]}
```

| key | |
|---|---|
| `emoji` | the emoji as Telegram holds it: no variation selector, so its heart is the bare `U+2764` |
| `count` | everyone who reacted with it, the account included |
| `chosen` | the account itself reacted with it |
| `recent_senders` | the **other** people Telegram names, at most three; always present, `[]` when it names none |
| `recent_senders[].id` | the user's id, as `sender_id` on a message; `null` for a reaction made as a chat (a channel, a group's anonymous admin) |
| `recent_senders[].name` | the user's display name or the chat's title; absent when TDLib does not know it |

- **The key is absent** for a message without reactions, and in a payload from a `tg` older than
  0.12.0.
- **The account is never in `recent_senders`**: `chosen` says it reacted. So
  `count - (chosen ? 1 : 0) - recent_senders.length` is the number of reactions nobody is named
  for. Telegram names at most three people per emoji and none in a large group.
- **A custom emoji is listed under the standard emoji it stands for** (`getCustomEmojiStickers`,
  which TDLib answers from its cache once it has seen the emoji) and joins that emoji's own entry
  when the message has one. One that names no emoji, or cannot be looked up within 5 s, is left
  out, with the reason on stderr.
- **Paid reactions are left out.**
- **In Saved Messages** reactions are the account's own tags, and are listed like any reaction.

**These are the reactions TDLib holds, and it does not hold all of them.** Telegram sends a
reaction change on its own only where it concerns the account:

- **A one-to-one chat:** every change arrives. TDLib itself never polls there; it relies on
  Telegram sending them.
- **A group or supergroup:** a reaction **to one of the account's own messages** arrives
  (Telegram's API: "Message authors will receive an updateMessageReactions update when a user
  reacts to their message"), and so does the account's own reaction made through `tg`. Whether
  the account's own reaction made on another device arrives is not established. A reaction
  between other people does not. A Telegram app shows those because it polls the messages on
  screen every 15 to 30 seconds; TDLib polls only messages reported with `viewMessages` in a chat
  opened with `openChat`, and `tg` calls neither.

So for other people's reactions to other people's messages in a group, `reactions` is what TDLib
last received with the message, which for a message it heard arrive is nothing. No
`message_reactions` event rings for them, and reading the message again (`message`, `sync
--reconcile-days`) answers from the same copy. Reading them would take the polling a Telegram app
does; `tg` does not do it.

## Testing

```bash
cargo test              # Run all tests
cargo test cli::tests   # Run CLI tests only
cargo clippy            # Lint
cargo fmt               # Format
```

## License

MIT
