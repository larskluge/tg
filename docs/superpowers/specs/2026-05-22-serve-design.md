# Long-Running Server Command (`tg serve`)

## Problem

Mycelium (and any other machine consumer) currently invokes `tg` one subcommand per call. Every call pays the full TDLib cold-start cost: client construction, FFI thread spawn, server handshake, chat-list bootstrap. For interactive workloads — alternating `whoami`, `chats`, `messages`, `send`, `download`, `search`, `mark-read`, `mark-unread` — this dominates wall-clock time and produces a noisy thrash of init/teardown work.

`tg sync` solved this for bulk message ingestion (one process, many chats), but only for that one verb. Everything else still pays cold start per call.

## Solution

A new `tg serve` subcommand: a long-running JSON-on-stdio service. One TDLib client is initialised at startup and reused for every request. The caller (typically `podman exec -i tg tg serve`) sends newline-delimited JSON requests on stdin and reads newline-delimited JSON responses on stdout. Requests are handled serially, in arrival order. Errors are reported in-band and never tear down the session. The session ends cleanly on stdin EOF or SIGTERM.

This is not a REPL for humans — there is no prompt, no readline, no plain-text output. It is a single-client JSON-RPC channel that piggybacks on stdio because that is the cheapest possible IPC inside a container.

## Wire Protocol

Newline-delimited JSON in both directions. Stdout is exclusively JSON; all human-readable output (TDLib logs, progress, warnings) is routed to stderr.

### Request

```json
{"id": "<opaque>", "cmd": "<command>", "args": { ... }}
```

- `id` (string, required): caller-chosen opaque correlation token. Echoed back on the matching response. The server does not interpret it.
- `cmd` (string, required): the command name (see [Command Set](#command-set)).
- `args` (object, optional): command-specific arguments. May be omitted when the command takes no arguments. Field names are snake_case and match the corresponding clap flag names (e.g. `since_utc`, `output_dir`, `oldest_first`).

### Response

Exactly one response is emitted for every request, in request-arrival order.

Success:

```json
{"id": "<echoed>", "ok": true, "result": <value>}
```

Failure:

```json
{"id": "<echoed>", "ok": false, "error": "<message>"}
```

- `result` shape matches the existing `--json` output of the corresponding subcommand (`MessageInfo`, `ChatInfo`, `SendResult`, `DownloadReport`, etc.). When today's CLI prints an array (e.g. `tg chats --json` prints `[ChatInfo, ...]`), `result` is that array. When it prints a single object, `result` is that object. When the CLI emits no payload (e.g. `tg mark-read`), `result` is `null`.
- `error` is a single human-readable string sourced from `TgError::to_string()`. No nested error envelope, no machine-readable error code in v1.

### Framing rules

- Each request and each response is a single line of UTF-8 JSON terminated by `\n`. No pretty-printing on the wire — `serde_json::to_string` (not `to_string_pretty`).
- A request that fails to parse as JSON, or whose top-level shape is invalid (missing `id`, missing `cmd`, etc.), produces an error response. The server attempts to extract `id` if present; if not, the response uses `"id": null`.
- A request with an unrecognised `cmd` produces `{"id": ..., "ok": false, "error": "unknown command: <cmd>"}`.
- Empty lines on stdin are ignored.
- Stdout is line-buffered (or explicitly flushed after each response) so the caller sees responses as they happen.

### Reserved for future use

The protocol reserves the shape `{"event": "<name>", "data": <value>}` (no `id`, no `ok`) for unsolicited server-to-client messages — TDLib update notifications, for example. The v1 server never emits these, but clients SHOULD ignore unrecognised top-level shapes rather than treating them as errors, so that future additions are non-breaking.

## CLI Interface

```
tg serve
```

No flags in v1. No `--json` (output is always JSON). The global `--json` flag is ignored if passed — the protocol is not negotiable.

Stdin: newline-delimited JSON requests.
Stdout: newline-delimited JSON responses.
Stderr: TDLib logs, parse-error notes, panic backtraces, anything humans might read.

## Command Set

Every existing read/write subcommand is reachable via `serve`. Mapping:

| `cmd`         | Args struct           | Result type                 | Notes                                              |
| ------------- | --------------------- | --------------------------- | -------------------------------------------------- |
| `whoami`      | _none_                | `UserInfo`                  |                                                    |
| `chats`       | `ChatsRequest`        | `Vec<ChatInfo>`             | `{ "limit": 50 }` default                          |
| `groups`      | `GroupsRequest`       | `Vec<ChatInfo>`             |                                                    |
| `unread`      | `UnreadRequest`       | `Vec<ChatInfo>`             |                                                    |
| `search`      | `SearchRequest`       | `Vec<ContactInfo>`          | `{ "query": "..." }`                               |
| `messages`    | `MessagesRequest`     | `Vec<MessageInfo>`          | accepts either `name` or `chat`; mirrors CLI rules |
| `send`        | `SendRequest`         | `SendResult`                | `--as` (bot send) is not supported by `serve` v1   |
| `download`    | `DownloadRequest`     | `DownloadReport`            | `output_dir` resolved inside the container         |
| `mark_read`   | `MarkReadRequest`     | `null`                      |                                                    |
| `mark_unread` | `MarkUnreadRequest`   | `null`                      |                                                    |
| `sync`        | `SyncRequest`         | `HashMap<String, SyncResult>` | HWM map moves into `args.hwm`; see [Sync](#sync) |

Notably **excluded**: `auth`, `auth bot`, `auth status`. These are interactive and/or modify on-disk credentials; the operator must stop `tg serve` before running them. Attempting `cmd: "auth"` returns `{"ok": false, "error": "auth is not available over serve; run \`tg auth\` directly"}`.

Bot sends (`--as`) are excluded from v1 because they use the HTTP API rather than the shared TDLib client; adding them is a clean follow-up.

### Sync

`sync` is the one command whose CLI form takes structured stdin. In `serve` mode that input moves into the request envelope:

```json
{"id": "s1", "cmd": "sync", "args": {
  "hwm": {"-1001666847309": 89508544512, "123456789": 0},
  "limit": 1000
}}
```

Today's `sync` writes per-chat results to stdout as a single pretty-printed JSON object after the run completes. In `serve`, the same map is the `result`. Progress lines that today go to stdout move to stderr (none exist today beyond the final dump, so this is mostly a no-op). The `--reconcile-days` flag becomes `args.reconcile_days: <u32 | null>`.

## Architecture

### New module: `src/commands/serve.rs`

Owns the request/response loop:

```rust
pub async fn run(client: &mut TdLibClient) -> Result<()>;
```

It:

1. Calls `client.start().await?` once at startup.
2. Spawns a stdin reader (either a blocking thread feeding a `tokio::sync::mpsc` channel, or `tokio::io::BufReader<Stdin>::lines()`).
3. Reads lines one at a time. For each line:
   a. Parses into `Request` (`{id, cmd, args}` with `serde_json::Value` for `args`).
   b. Dispatches to a per-command handler.
   c. Writes the response line to stdout and flushes.
4. On stdin EOF or SIGTERM: breaks the loop and returns. `main.rs` always calls `client.shutdown().await` afterwards.

### Handler factoring

Today, `main.rs::run_command` couples three things per command: clap-arg extraction, target/recipient resolution, and the actual TDLib call. `serve` needs the second and third parts without the first.

Refactor each `commands/<name>.rs` to expose a function that takes a typed `<Name>Request` struct (or primitive args, where the existing handler already does) and returns a `Result<<ResultType>>`. Both code paths call that function:

```
clap args  ──► from_cli ──►  ChatsRequest ──┐
                                            ├──► chats::handle(&client, req) ──► Vec<ChatInfo>
serve args ──► serde_json::from_value ──────┘
```

Where the existing handler already takes primitive args (e.g. `messages::get_messages(client, target, limit, since_utc, oldest_first)`), the new `handle` function is a thin wrapper that constructs those primitives from the request struct. No semantic changes to the inner handlers.

The `<Name>Request` structs live in a new `src/commands/request.rs` (or per-command, beside each handler — to be decided in the plan). They derive `Deserialize`, with `#[serde(default)]` on optional fields so omitted keys behave like the CLI defaults (e.g. `limit: 50`).

The clap `*Args` structs in `cli.rs` are unchanged. Conversions live in `main.rs` (or beside each handler) and are mechanical: `ChatsArgs { limit } → ChatsRequest { limit }`.

### Dispatcher

A single `match cmd.as_str()` in `serve.rs` routes to handlers:

```rust
let result: Result<serde_json::Value> = match req.cmd.as_str() {
    "whoami"      => to_value(whoami::handle(client).await?),
    "chats"       => to_value(chats::handle(client, from_value(req.args)?).await?),
    "messages"    => to_value(messages::handle(client, from_value(req.args)?).await?),
    // ...
    "auth" | "auth_bot" | "auth_status" => Err(TgError::Other(
        "auth is not available over serve; run `tg auth` directly".into())),
    other => Err(TgError::Other(format!("unknown command: {other}"))),
};
```

Where `to_value` uses `serde_json::to_value` and `()` → `Value::Null`.

### Concurrency

Strictly serial. The dispatcher awaits each handler to completion before reading the next line. This matches the current `main.rs::run_command` model exactly — no new concurrency invariants for TDLib to worry about. A slow `download` blocks subsequent requests; mycelium is expected to tear down the session and respawn if a command hangs (per the brief).

### Stdout discipline

Two rules:

1. Every byte written to stdout is part of a JSON response line, ending in `\n`. Nothing else.
2. All other output — handler-internal `eprintln!`s, panic messages, TDLib chatter — goes to stderr.

To enforce rule 2, audit existing handlers for any `println!` / `print!` calls and either remove them or route them through `eprintln!`. A quick grep shows the suspects are:

- `messages::run` prints an `eprintln!` already when filtering returns empty (fine).
- `sync` currently does its final dump via `println!` from `main.rs` — that's the `main.rs` caller's responsibility, so handlers themselves are clean.

If any future handler prints to stdout it will silently corrupt the channel. A simple `#![deny(clippy::print_stdout)]` lint on `commands/` would catch this; whether to add it is left to the plan.

### Lifecycle

Startup:

1. `main.rs` loads credentials and constructs `TdLibClient` (same as today).
2. Routes `Command::Serve` to `serve::run(&mut client)`.
3. `serve::run` calls `client.start().await?` and enters the loop.

Shutdown triggers:

- **Stdin EOF** (caller closed the pipe): break the loop cleanly.
- **SIGTERM / SIGINT**: install a `tokio::signal` handler that flips an `AtomicBool`; the loop checks it between requests. In-flight request is allowed to complete (per "tear down + respawn for hangs" — graceful shutdown does not pre-empt a long `download`). For SIGKILL or container kill, the process dies and TDLib state stays consistent on disk because TDLib commits incrementally.

In all clean exits, `main.rs` calls `client.shutdown().await` (the existing path). Exit code `0`.

### Container assumption

`podman exec -i tg tg serve` runs a new `tg serve` instance per `exec` call inside the running container. The expected usage is: mycelium holds **one** long-lived `serve` child (one `podman exec -i`), writes many requests over its lifetime, and closes stdin when shutting down. If mycelium restarts, it gets a cold TDLib but TDLib's on-disk session survives — auth is preserved. This document does not change anything about the container itself; the existing `Containerfile` already builds `tg`, and `tg serve` is just another subcommand.

## Errors

- Parse error on a request line → `{"id": <if extractable, else null>, "ok": false, "error": "invalid request: <serde message>"}`. Loop continues.
- Unknown `cmd` → `{"id": ..., "ok": false, "error": "unknown command: <cmd>"}`. Loop continues.
- Handler returns `Err(TgError::...)` → `{"id": ..., "ok": false, "error": "<TgError to_string>"}`. Loop continues.
- Catastrophic TDLib failure (e.g. authentication revoked mid-session): handlers will return `Err`; subsequent requests will likely also fail. The server does not attempt to re-initialise TDLib. The operator decides whether to kill and restart.

## Out of Scope (v1)

- Authentication commands (`auth`, `auth bot`, `auth status`).
- Bot sends (`send --as <bot>`).
- Streaming responses (multiple frames per request id). All responses are single frames.
- Per-request cancellation or timeout. Caller tears down + respawns.
- Multi-client concurrency (multiple parallel `podman exec` sessions). Single client only; document it in the README.
- Update notifications (TDLib `updateNewMessage` etc.). Protocol leaves room for `{"event": ...}` frames but the v1 server never emits any.
- Machine-readable error codes / typed error envelopes. Strings only in v1.

## Testing

### Unit (in `src/commands/serve.rs`)

Dispatcher tests using a `MockClient` (already exists in `src/client.rs`'s `mock` module):

- Request with valid `cmd: "whoami"` returns `{"ok": true, "result": {...}}` with `id` echoed.
- Request with unknown `cmd` returns `{"ok": false, "error": "unknown command: ..."}` and the loop continues to process the next request.
- Request with malformed JSON returns `{"ok": false, "error": "invalid request: ..."}` with `id: null`.
- Request that triggers a handler error returns `{"ok": false}` with the error message verbatim.
- Multiple requests are processed in arrival order; response `id`s match input `id`s in order.

These tests drive `serve::dispatch_one(client, line) -> String` (a pure function) directly, no real stdio.

### Integration (in `tests/`)

A new `tests/serve_integration.rs` (`#[cfg(feature = "integration")]` or behind a `--ignored` marker — to be decided in the plan, mirroring whatever convention `tests/` already uses):

1. Spawn `tg serve` as a child process via `std::process::Command` with piped stdin/stdout.
2. The test uses the mock-backed code paths where possible; if a real TDLib is required, gate the test behind an env var (`TG_SERVE_INTEGRATION=1`) so CI doesn't try to talk to Telegram.
3. Write two requests (`whoami`, `chats --limit 1`) to stdin.
4. Read two response lines from stdout.
5. Assert: both responses parse as JSON, `id` values match request order, `ok: true`, expected `result` shape.
6. Close stdin. Wait for process exit. Assert exit code 0.

If standing up a real TDLib in CI is impractical, the integration test reuses the existing test harness pattern (whatever `tests/` already does for end-to-end coverage) and the round-trip assertions stay; the inner data is mocked.

## README updates

The existing README has a "Bulk sync" section that documents `tg sync` as the machine-use entry point. Replace it with a broader **"Machine use"** section that:

1. Leads with `tg serve` as the preferred entry point for any long-running caller: explains the wire protocol, the request/response shape, lifecycle, error semantics, and the single-client constraint.
2. Retains `tg sync` as a one-shot variant for callers that only need bulk message ingestion and don't want to maintain a long-lived child.
3. Documents the operational constraint that `tg auth` must be run with the `serve` session stopped, since both want exclusive access to TDLib's on-disk database.
4. Shows the `podman exec -i tg tg serve` invocation pattern with a sample request/response exchange.

The existing "Bulk sync" example (`echo '{"123": 42}' | tg sync`) stays, but moves under a "One-shot bulk sync" subheading.

## File touch list

- `src/cli.rs` — add `Command::Serve` variant (no args).
- `src/main.rs` — route `Command::Serve` to `serve::run`; ensure clean shutdown path still runs.
- `src/commands/mod.rs` — export new `serve` module.
- `src/commands/serve.rs` — new. Request/response types, dispatcher, loop, signal handling.
- `src/commands/request.rs` (or per-command request structs) — new. `Deserialize`-derived request structs and `From<*Args>` impls.
- `src/commands/{chats,groups,unread,messages,search,send,download,mark_read,mark_unread,sync,whoami}.rs` — add or expose a `handle(client, req) -> Result<...>` function for each. Existing internals untouched.
- `tests/serve_integration.rs` — new.
- `README.md` — rewrite "Bulk sync" section as "Machine use".

No changes to `auth.rs`, `bot_api.rs`, `client.rs`, `output.rs`, `credentials.rs`, `resolve.rs`, `error.rs`.
