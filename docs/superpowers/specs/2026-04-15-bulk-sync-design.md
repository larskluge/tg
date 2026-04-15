# Bulk Sync Command (`tg sync`)

## Problem

Mycelium syncs Telegram messages by spawning a separate `tg messages` process per chat, serialized through a mutex (TDLib's database allows only one caller). Each invocation pays the full TDLib startup/teardown cost. For N chats, this means N init cycles — the primary bottleneck.

## Solution

A new `tg sync` command that accepts a batch of chat IDs with per-chat high-water mark message IDs via stdin, fetches messages for all chats within a single TDLib session, and outputs results as JSON to stdout. One process startup serves all chats.

## CLI Interface

```
tg sync [--reconcile-days N] [--limit N] < hwm.json
```

### Stdin

Object mapping chat ID (string) to last seen message ID (integer):

```json
{
  "-1001666847309": 89508544512,
  "123456789": 42
}
```

A value of `0` means "no prior state — fetch latest messages up to limit".

### Flags

- `--reconcile-days N`: Overrides all per-chat HWMs with a timestamp-based boundary computed from `now - N days`. Used for periodic reconciliation sweeps that detect edits and deletions. Stdin is still required (to specify which chats to sync); the HWM values in stdin are ignored when this flag is set.
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
- The HWM boundary message itself is **excluded** from results (it was already ingested).

### Exit codes

- `0`: All chats succeeded.
- `1`: At least one chat had an error (successful results still in output).

## Architecture

### New file: `src/commands/sync.rs`

Core function:

```rust
pub async fn sync_chats<C: TelegramClient>(
    client: &C,
    hwm_map: HashMap<i64, i64>,  // chat_id -> last seen message ID
    limit: i32,
    reconcile_days: Option<u32>,
) -> HashMap<i64, SyncResult>
```

Result type:

```rust
pub enum SyncResult {
    Messages(Vec<MessageInfo>),
    Error { error: String },
}
```

### Flow

**Normal sync (message-ID HWMs):**

1. Parse stdin JSON into `hwm_map` (`HashMap<i64, i64>`).
2. For each `(chat_id, hwm_message_id)`:
   a. Call `client.get_messages(chat_id, limit, Some(hwm_message_id))` — fetches messages down to (and including) the boundary.
   b. Strip the boundary message from results (already ingested).
   c. On error: store `SyncResult::Error`, continue to next chat.
3. Serialize the full result map to stdout as JSON.

**Reconcile sweep (`--reconcile-days N`):**

1. Compute `now - N days` as a Unix timestamp.
2. For each chat: use timestamp-based boundary lookup (`get_boundary_message_id`) with warmup fetch and retry, same as `tg messages --since-utc`.
3. Otherwise same as normal flow.

### Reused components

No new `TelegramClient` trait methods required:

- `get_messages` with `until_message_id` — already supports message-ID-based pagination boundary.
- `get_boundary_message_id` — used only for `--reconcile-days` timestamp-to-message-ID conversion.
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

1. **Happy path**: Multiple chats with message-ID HWMs, verify messages returned.
2. **Boundary stripping**: HWM message itself is excluded from results.
3. **HWM zero**: Fetches all messages (no boundary).
4. **Partial failure**: One chat errors, others succeed.
5. **Reconcile override**: `--reconcile-days 7` uses timestamp path regardless of message-ID HWMs.
6. **Stdin parsing**: Valid input, empty object, malformed JSON, non-numeric chat IDs.
7. **Serialization**: `SyncResult` enum serializes correctly (array vs error object).

## Error Handling

| Scenario | Behavior |
|---|---|
| Invalid stdin JSON | Exit 1 with error to stderr, no stdout |
| Empty stdin object `{}` | Exit 0, output `{}` |
| Individual chat fetch fails | `{"error": "..."}` for that chat, continue others |
| TDLib auth not ready | Exit 1 with "Not authenticated" to stderr |
