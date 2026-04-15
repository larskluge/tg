# Bulk Sync Command (`tg sync`)

## Problem

Mycelium syncs Telegram messages by spawning a separate `tg messages` process per chat, serialized through a mutex (TDLib's database allows only one caller). Each invocation pays the full TDLib startup/teardown cost. For N chats, this means N init cycles — the primary bottleneck.

## Solution

A new `tg sync` command that accepts a batch of chat IDs with per-chat high-water mark timestamps via stdin, fetches messages for all chats within a single TDLib session, and outputs results as JSON to stdout. One process startup serves all chats.

## CLI Interface

```
tg sync [--reconcile-days N] [--limit N] < hwm.json
```

### Stdin

Object mapping chat ID (string) to ISO 8601 HWM timestamp:

```json
{
  "-1001666847309": "2026-04-15T00:00:00Z",
  "123456789": "2026-04-14T12:00:00Z"
}
```

### Flags

- `--reconcile-days N`: Overrides all per-chat HWMs with `now - N days`. Used for periodic reconciliation sweeps that detect edits and deletions.
- `--limit N`: Max messages per chat. Default: 1000.
- No `--json` flag — output is always JSON (this command is machine-only).

### Stdout

Object keyed by chat ID string. Each value is either an array of message objects (same shape as `tg messages --json`) or an error object:

```json
{
  "-1001666847309": [
    {"id": 100, "chat_id": -1001666847309, "sender": "Alice", "text": "hello", "date": "2026-04-15T01:00:00Z", ...},
    {"id": 101, "chat_id": -1001666847309, "sender": "Bob", "text": "hi", "date": "2026-04-15T01:01:00Z", ...}
  ],
  "123456789": [],
  "999999999": {"error": "Chat not found"}
}
```

- Chats with no new messages: empty array `[]`.
- Chats that fail: `{"error": "reason"}`.

### Exit codes

- `0`: All chats succeeded.
- `1`: At least one chat had an error (successful results still in output).
- `2`: Fatal error before processing (invalid stdin JSON, auth failure). Error to stderr, no stdout.

## Architecture

### New file: `src/commands/sync.rs`

Core function:

```rust
pub async fn sync_chats<C: TelegramClient>(
    client: &C,
    hwm_map: HashMap<i64, String>,  // chat_id -> ISO 8601 timestamp
    limit: i32,
    reconcile_days: Option<u32>,
) -> Result<HashMap<i64, SyncResult>>
```

Result type:

```rust
pub enum SyncResult {
    Messages(Vec<MessageInfo>),
    Error(String),
}
```

### Flow

1. Parse stdin JSON into `hwm_map`.
2. If `--reconcile-days N`, compute `now - N days` as ISO 8601 and replace all HWM values.
3. For each `(chat_id, hwm)` in the map:
   a. Parse the HWM timestamp to epoch seconds.
   b. Call `client.get_boundary_message_id(chat_id, timestamp)` to find the message boundary.
   c. Call `client.get_messages(chat_id, limit, boundary_msg_id)` to fetch messages after the boundary.
   d. On success: store `SyncResult::Messages(msgs)`.
   e. On error: store `SyncResult::Error(err.to_string())`, continue to next chat.
4. Serialize the full result map to stdout as JSON.

### Reused components

No new `TelegramClient` trait methods required. The command composes existing primitives:

- `get_boundary_message_id` — warmup fetch + retry for stale index.
- `collect_messages_paginated` (via `get_messages`) — batching, dedup, boundary cutoff.
- `MessageInfo` serialization — existing JSON output support.

### CLI definition in `cli.rs`

```rust
/// Bulk-sync messages for multiple chats (machine use)
Sync(SyncArgs),
```

```rust
pub struct SyncArgs {
    #[arg(long)]
    pub reconcile_days: Option<u32>,
    #[arg(long, default_value = "1000")]
    pub limit: i32,
}
```

## Testing

Unit tests in `sync.rs` using the mock `TelegramClient` (same pattern as existing commands):

1. **Happy path**: 3 chats with different HWMs, mock returns appropriate messages. Verify output map has correct messages per chat.
2. **Empty results**: Chat with HWM newer than all messages returns empty array.
3. **Partial failure**: One chat errors, others succeed. Verify failed chat gets `SyncResult::Error`, others get messages.
4. **Reconcile override**: `--reconcile-days 7` replaces all HWMs with `now - 7 days`.
5. **Stdin parsing**: Malformed JSON, empty object, non-numeric chat IDs return clear errors.

## Error Handling

| Scenario | Behavior |
|---|---|
| Invalid stdin JSON | Exit 2 with error to stderr, no stdout |
| Empty stdin object `{}` | Exit 0, output `{}` |
| Individual chat fetch fails | `{"error": "..."}` for that chat, continue others |
| Boundary lookup fails (stale index) | Already retried internally; if still fails, treat as chat error |
| TDLib auth not ready | Exit 1 with "Not authenticated" to stderr |
