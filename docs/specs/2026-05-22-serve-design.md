# Long-Running Server Command (`tg serve`)

## Problem

Every `tg <cmd>` invocation pays the full TDLib cold-start cost: client construction, FFI thread spawn, server handshake, chat-list bootstrap. For interactive workloads — alternating `whoami`, `chats`, `messages`, `send`, `download`, `search`, `mark-read`, `mark-unread` — this dominates wall-clock time.

`tg sync` solved this for bulk message ingestion (one process, many chats) by jamming everything into one subcommand. That doesn't generalise: every other CLI invocation still cold-starts.

## Solution

A new `tg serve` subcommand: a long-running background process whose sole job is to keep one TDLib client warm and expose it over a Unix domain socket. Every other `tg <cmd>` automatically detects the socket and forwards its request to the server; the server processes it against the warm TDLib client and returns the result; the client renders that result the same way it would have if it had executed locally. The user-visible UX of `tg messages`, `tg chats`, etc. is identical — only faster.

When the socket isn't present (no `tg serve` running, or it crashed), CLI commands fall back to today's in-process TDLib path. `tg serve` is a pure performance optimisation; nothing breaks without it.

## Wire Protocol

Newline-delimited JSON over a Unix socket. One connection may carry one request (typical for CLI clients) or many requests in arrival order (long-lived clients like mycelium). The framing is the same in both cases.

### Request

```json
{"id": "<opaque>", "cmd": "<command>", "args": { ... }}
```

- `id` (string, required): caller-chosen correlation token. Echoed back on the matching response. CLI clients can use any value (e.g. `"1"`); pipelining clients pick unique values per in-flight request.
- `cmd` (string, required): the command name (see [Command Set](#command-set)).
- `args` (object, optional): command-specific arguments. May be omitted when the command takes no arguments. Field names are snake_case and match the corresponding clap flag names (e.g. `since_utc`, `output_dir`, `oldest_first`).

### Response

Exactly one response per request, in arrival order on the connection.

Success:

```json
{"id": "<echoed>", "ok": true, "result": <value>}
```

Failure:

```json
{"id": "<echoed>", "ok": false, "error": "<message>"}
```

- `result` shape matches the existing `--json` output of the corresponding subcommand (`MessageInfo`, `ChatInfo`, `SendResult`, `DownloadReport`, etc.). When today's CLI prints an array, `result` is that array. When it prints a single object, `result` is that object. When the CLI emits no payload (e.g. `tg mark-read`), `result` is `null`.
- `error` is a single human-readable string sourced from `TgError::to_string()`. No nested envelope, no machine-readable error code in v1.

### Framing rules

- Each request and each response is a single line of UTF-8 JSON terminated by `\n`. No pretty-printing on the wire — `serde_json::to_string` (not `to_string_pretty`).
- A request that fails to parse, or whose top-level shape is invalid, produces an error response. The server attempts to extract `id` if present; if not, the response uses `"id": null`.
- A request with an unrecognised `cmd` produces `{"id": ..., "ok": false, "error": "unknown command: <cmd>"}`.
- Empty lines are ignored.
- Stdout/socket writes are flushed after each response.

### Reserved for future use

The protocol reserves the shape `{"event": "<name>", "data": <value>}` (no `id`, no `ok`) for unsolicited server-to-client messages — TDLib update notifications, for example. The v1 server never emits these, but clients SHOULD ignore unrecognised top-level shapes rather than treating them as errors, so that future additions are non-breaking.

## Socket

### Location

Resolved at startup (server and client agree on the same rule):

1. If `TG_SERVE_SOCKET` is set and non-empty: use that absolute path verbatim.
2. Otherwise, if `XDG_RUNTIME_DIR` is set: `$XDG_RUNTIME_DIR/tg.sock`.
3. Otherwise: `dirs::data_dir()/tg/serve.sock` (e.g. `~/Library/Application Support/tg/serve.sock` on macOS).

The server creates parent directories as needed with permissions `0700`, and the socket itself with permissions `0600`. The socket is owned by the user running `tg serve`; no other user can connect.

If `TG_SERVE_SOCKET` is set to the empty string (`TG_SERVE_SOCKET=`), CLI clients treat the server as disabled and always use in-process TDLib. The server itself refuses to start with an empty `TG_SERVE_SOCKET`.

### Lifecycle

**Startup:**

1. Resolve socket path.
2. Check if a socket file already exists at that path. If it does, attempt to connect to it briefly:
   - If the connection succeeds: another `tg serve` is already running. Exit with code `1` and print `tg serve: already running at <path>` to stderr.
   - If the connection is refused: the socket is stale (previous server died). `unlink()` it and proceed.
3. Construct `TdLibClient`, call `client.start().await?`, wait for sync.
4. `bind()` the socket, `listen()`.
5. Print `tg serve: listening at <path>` to stderr (informational; stdout stays clean for symmetry with the rest of the tool).
6. Accept connections in a loop.

**Per-connection:**

- Read NDJSON requests until EOF. For each request, take the TDLib mutex, dispatch the handler, release the mutex, write the response line, flush.

**Shutdown:**

- SIGTERM / SIGINT: stop accepting new connections, finish any in-flight requests, call `client.shutdown().await`, unlink the socket, exit `0`.
- Existing connections may be dropped abruptly if shutdown signals arrive during their requests; clients reading those responses get an EOF on the socket and surface a connection error.

### Concurrency

The server accepts many connections concurrently but serialises TDLib calls through a single `tokio::sync::Mutex<TdLibClient>`. This matches TDLib's single-writer assumption and the current CLI semantics. A slow `download` blocks other requests until it completes; callers tear down + retry (or wait) — see [Out of Scope](#out-of-scope-v1).

## Client-Side Routing

Every existing subcommand (except `auth`) is augmented to try the socket first:

```
parse clap args
  ↓
construct <Cmd>Request   (typed, serde-Deserialize)
  ↓
try connect to socket
  ├─ success: send request, read response, render result, exit
  └─ refused/missing:
       fall back to today's in-process path
```

The decision is made once at the top of each subcommand handler in `main.rs`. The same `<Cmd>Request` struct used by the server is used to build the request envelope on the client; the server deserialises it and dispatches to the same handler function.

Result rendering happens on the client side. The client receives the response JSON, deserialises `result` into the appropriate typed struct (`Vec<ChatInfo>`, `MessageInfo`, `DownloadReport`, etc.), and feeds it to the existing `print_*` helpers — so the `--json` vs plain-text choice, table layout, terminal width detection, and ANSI colour decisions all stay where they are today.

### Path resolution

Commands that take filesystem paths must resolve them on the client side before sending, because the server may have a different CWD. Specifically:

- `tg download --output-dir <path>`: the client canonicalises `<path>` to an absolute path (`std::env::current_dir().join(path)`) and sends the absolute form in `args.output_dir`. The server writes there.

### Auth

`tg auth`, `tg auth bot`, `tg auth status` all bypass the socket entirely. Before running their normal flow, they check whether `tg serve` is running (by attempting a connection); if so they fail with:

```
tg auth: cannot run while `tg serve` is active.
Stop the serve process and retry.
```

Reason: TDLib needs exclusive write access to its on-disk database, and `tg auth` modifies credentials anyway — running it through the server would mean re-auth doesn't take effect until the server restarts.

### Send `--as` (bot send)

Bot sends use the HTTP API, not TDLib, so they don't benefit from the warm client. `tg send --as <bot>` ignores the socket and runs in-process today's way. The non-bot `tg send` does route through the socket.

## Command Set

The server accepts every subcommand whose handler talks to TDLib:

| `cmd`         | Args struct           | Result type                   |
| ------------- | --------------------- | ----------------------------- |
| `whoami`      | _none_                | `UserInfo`                    |
| `chats`       | `ChatsRequest`        | `Vec<ChatInfo>`               |
| `groups`      | `GroupsRequest`       | `Vec<ChatInfo>`               |
| `unread`      | `UnreadRequest`       | `Vec<ChatInfo>`               |
| `search`      | `SearchRequest`       | `Vec<ContactInfo>`            |
| `messages`    | `MessagesRequest`     | `Vec<MessageInfo>`            |
| `send`        | `SendRequest`         | `SendResult`                  |
| `download`    | `DownloadRequest`     | `DownloadReport`              |
| `mark_read`   | `MarkReadRequest`     | `null`                        |
| `mark_unread` | `MarkUnreadRequest`   | `null`                        |
| `sync`        | `SyncRequest`         | `HashMap<String, SyncResult>` |

Notably excluded: `auth`, `auth bot`, `auth status`, `send --as <bot>`. These run in-process always.

For `sync`, the HWM map that today comes from stdin moves into `args.hwm`:

```json
{"id": "s1", "cmd": "sync", "args": {
  "hwm": {"-1001666847309": 89508544512, "123456789": 0},
  "limit": 1000
}}
```

When `tg sync` is run as a client (i.e. `tg serve` is up), it reads the same HWM JSON from stdin as today, packages it into `args.hwm`, sends the request, and prints the response `result` to stdout exactly as today.

## Architecture

### New module: `src/commands/serve.rs`

Owns the listener loop and dispatch table:

```rust
pub async fn run(client: TdLibClient) -> Result<()>;
```

It:

1. Resolves the socket path, handles stale-socket cleanup.
2. Wraps the client in `Arc<Mutex<TdLibClient>>`.
3. Calls `client.start().await?` (under the mutex, once).
4. `bind()`s the Unix socket and starts accepting.
5. Spawns a task per connection. Each task reads NDJSON lines, dispatches under the mutex, writes responses.
6. Installs a SIGTERM/SIGINT handler that triggers clean shutdown.

The dispatch function is the unit-testable seam:

```rust
pub async fn dispatch(client: &TdLibClient, req: Request) -> Response;
```

Where `Request { id, cmd, args }` and `Response { id, ok, result | error }`.

### New module: `src/client_proxy.rs` (name TBD)

Owns the client-side "try socket first, else in-process" decision:

```rust
pub enum Transport {
    Socket(UnixStream),
    InProcess(TdLibClient),
}

pub async fn connect() -> Transport;
pub async fn request<R: Serialize, T: DeserializeOwned>(
    transport: &mut Transport,
    cmd: &str,
    args: R,
    in_process: impl FnOnce(&TdLibClient) -> Future<Output = Result<T>>,
) -> Result<T>;
```

The exact shape of this abstraction is a plan-level decision — the cleanest version is probably a small `Transport` enum plus per-command thin wrappers in `main.rs`. The important invariant is that the **handler logic only lives in one place** (`commands/<name>.rs::handle`) and is called either by the server (on TDLib) or by the in-process fallback (also on TDLib).

### Handler factoring

Today, `main.rs::run_command` couples three things per command: clap-arg extraction, target resolution, and the TDLib call. The new design needs the second and third reusable in two contexts (server-side dispatch, client-side fallback) without the first.

Refactor each `commands/<name>.rs` to expose:

```rust
pub async fn handle(
    client: &TdLibClient,
    req: <Name>Request,
) -> Result<<ResultType>>;
```

Both the server dispatcher and the in-process fallback in `main.rs` call this. The `<Name>Request` structs derive `Deserialize` and `Serialize` (the latter so clients can build wire requests). They live next to their handlers (e.g. `commands/chats.rs` defines `ChatsRequest`).

The clap `*Args` structs in `cli.rs` stay; conversion `*Args → *Request` is a mechanical `From` impl alongside each request struct.

### Output rendering

Unchanged. `output.rs::print_chats_table`, `print_messages_table`, `print_output`, `print_list` continue to take typed structs. The client deserialises the response `result` into the right typed struct and feeds it in, regardless of whether the result came from the socket or from in-process TDLib.

## Errors

- Socket connect fails (refused, missing, permission denied): client silently falls back to in-process TDLib. No warning printed.
- Socket connects but write fails partway: client surfaces `"error: tg serve connection dropped"` and exits non-zero. We do not retry by falling back, because TDLib state has already been mutated server-side for write commands.
- Server-side request parse error: `{"ok": false, "error": "invalid request: <serde message>"}`. Connection stays open for further requests.
- Server-side unknown `cmd`: `{"ok": false, "error": "unknown command: <cmd>"}`.
- Server-side handler error: `{"ok": false, "error": "<TgError to_string>"}`.
- Server detects another instance already running on the socket path: exits 1 with `tg serve: already running at <path>`.

## Out of Scope (v1)

- Authentication commands over the socket.
- Bot sends (`send --as <bot>`) over the socket.
- Per-request cancellation or timeouts. A hung `download` blocks the channel; caller decides whether to wait or kill+restart the server.
- Streaming responses (multiple frames per request id). All responses are single frames.
- TLS / authentication on the socket itself. Filesystem permissions (`0600`) are the access boundary.
- Cross-host or network transports. Unix socket only.
- Update notifications (TDLib `updateNewMessage` etc.). Protocol reserves `{"event": ...}` for them; v1 server never emits any.
- Machine-readable error codes / typed error envelopes. Strings only in v1.
- Automatic server startup (no systemd unit, no launchd plist). Operator runs `tg serve` themselves (or via the container's entrypoint).
- Hot reload / re-auth without restart. Re-running `tg auth` requires stopping `tg serve` first.

## Testing

### Unit (`src/commands/serve.rs`)

Dispatcher tests using `MockClient`:

- Valid `cmd: "whoami"` → `{ok: true, result: ...}` with `id` echoed.
- Unknown `cmd` → `{ok: false, error: "unknown command: ..."}`, loop continues.
- Malformed JSON → `{ok: false, error: "invalid request: ..."}`, `id: null`.
- Handler error → `{ok: false, error: ...}` with message verbatim.
- Sequential requests on one connection: responses arrive in order with matching `id`s.
- Concurrent connections: requests serialise through the mutex; per-connection ordering preserved.

These drive `serve::dispatch` directly, no real sockets.

### Unit (client-side routing)

Test the `Transport` selection in isolation: given a socket path that exists vs. doesn't exist, the proxy resolves to `Transport::Socket` vs. `Transport::InProcess`. Use a real `UnixListener` bound to a tempdir path for the socket case (no TDLib required).

### Integration (`tests/serve_integration.rs`)

Spawn `tg serve` as a child process pointed at a tempdir socket via `TG_SERVE_SOCKET`. Use the `MockClient`-backed binary if feasible; otherwise gate on `TG_SERVE_INTEGRATION=1` env var.

Scenarios:

1. **Round-trip on one connection.** Open a Unix socket, write `whoami` + `chats --limit 1` requests, read two response lines, assert `id`s match request order, `ok: true`, expected shapes.
2. **Parallel connections.** Open two sockets, send `chats` on each concurrently, both succeed; the server's mutex serialised them but both got correct responses.
3. **Stale socket cleanup.** Pre-create a dead socket file at the target path. Server starts, detects no listener, unlinks, binds successfully.
4. **Already-running detection.** Start one `tg serve`, then start a second pointed at the same socket. Second exits `1` with the "already running" message; first keeps serving.
5. **Clean shutdown.** Send SIGTERM to a running server. It finishes any in-flight request, unlinks the socket, exits `0`.
6. **CLI fallback.** With no `tg serve` running, run a `tg whoami` (mocked-TDLib build) and assert it works the same as today.

## README updates

The existing README has a "Bulk sync" section that documents `tg sync`. Replace it with a broader **"Machine use"** section that:

1. Leads with `tg serve`: what it does (keeps TDLib warm), how to start it (`tg serve` or `podman exec -d tg tg serve` for backgrounded use), where the socket lives, the `TG_SERVE_SOCKET` override.
2. Explains the transparent client-side routing: every other `tg <cmd>` automatically uses the server if it's up, otherwise behaves exactly as today.
3. Documents the wire protocol for external (non-`tg`-CLI) clients that want to speak NDJSON directly.
4. Documents the operational constraint that `tg auth` must be run with `tg serve` stopped.
5. Retains `tg sync` as a bulk-ingest variant that works whether or not the server is up (with the server, it's just a single request through the same socket).

The existing `echo '{"123": 42}' | tg sync` example stays under a "Bulk sync" subheading.

## File touch list

- `src/cli.rs` — add `Command::Serve` variant (no args).
- `src/main.rs` — route `Command::Serve` to `serve::run`; refactor every other command's branch to go through the client-proxy abstraction.
- `src/commands/mod.rs` — export new `serve` module.
- `src/commands/serve.rs` — new. Socket listener, dispatcher, signal handling.
- `src/client_proxy.rs` (name TBD) — new. Transport detection + per-command request/response helpers.
- `src/commands/{chats,groups,unread,messages,search,send,download,mark_read,mark_unread,sync,whoami}.rs` — add `handle(client, req)` function and per-command `*Request` struct (Serialize + Deserialize) and `From<*Args>` impl.
- `src/auth.rs` — add a "is serve running" precheck that errors out cleanly with the stop-serve instruction.
- `tests/serve_integration.rs` — new.
- `README.md` — rewrite "Bulk sync" section as "Machine use".

No changes to `bot_api.rs`, `client.rs`, `output.rs`, `credentials.rs`, `resolve.rs`, `error.rs`.
