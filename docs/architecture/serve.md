# `tg serve`

The serve socket's live events and its requests. The rules every change keeps are in
[AGENTS.md](../../AGENTS.md); this file is how it works as built.

**Subscriptions (`subscribe` / `tg stream`, since 0.11.0):** the frames, names and exit codes
are a contract with Mycelium's Telegram consumer (its spec:
`docs/specs/2026-09-30-realtime-message-streaming-design.md` in mycelium) and are documented in
`docs/specs/2026-05-22-serve-design.md` → *Event frames*. What the code relies on:

- **A subscription never takes the client lock.** `serve::run` takes the update channel
  (`TdLibClient::updates()`) before the client goes behind its mutex and hands each connection
  a `Subscriptions`. `handle_connection` answers `subscribe` itself, so `serve::dispatch` never
  sees it (it refuses the command outright if called directly). Pinned by
  `subscribe_acks_then_streams_events_without_the_client_lock`, which holds the lock throughout.
- **Attach, then ack.** The broadcast receiver is created before the ack is written, so nothing
  after the ack can be missed.
- **Shutdown ends subscriptions first**, through a `watch` switch. A subscription never ends on
  its own, so without it every live subscriber held the drain for its full 5 s, on top of
  TDLib's close. Measured locally with one subscriber: `tg serve` stops in 2.1 s.
- **The peer's close is read, not written.** A subscription reads (and discards) the peer's
  input to see EOF, so a half-close ends it: `serve_client::stream` keeps its write half alive
  for exactly that reason. A peer that vanishes without a close is found by the heartbeat write.
- **The channel is lossy by design.** `UPDATE_CHANNEL_CAPACITY` (1024) bounds how far a
  subscriber may fall behind; past it tokio drops updates for that receiver alone, the sender
  never blocks, and the subscriber gets `lagged`. The short-lived receivers (auth, send
  confirmations) share the same channel and gain from the larger capacity.
- **Loading chats rings every chat once.** The first `chats`/`groups`/`unread` request of a
  serve session makes TDLib announce each loaded chat's last message: ~405
  `chat_last_message` frames from one `tg chats --limit 2` on 2026-09-30, and none from the
  requests after it.
- **Supergroups and channels may not ring.** TDLib's `openChat` docs: "all updates are received
  only for opened chats" in them. Only the reaction poll calls `openChat`, for a moment per
  group and interval (see **Reactions** in [reactions.md](reactions.md)), and channels never; how many synced groups ring
  in between is to be measured on outpost.
- **`tg stream` has no in-process fallback**, unlike every other command: a second TDLib client
  on serve's database is the one thing it must never open. It exits 1 when serve is
  unreachable or ends the stream, and 0 when its stdout closes **or its stdin reaches EOF**.
- **Stdin's EOF is the lifetime signal under `podman exec -i`**, not stdout. Measured with
  podman 6.1.2: when the exec client goes away (reader closed, SIGTERM, SIGKILL), the process in
  the container keeps running because conmon keeps accepting its stdout, but its stdin is closed
  in every case. `serve_client::stdin_closed` reads stdin on a plain, never-joined thread:
  tokio's `stdin()` cannot be cancelled and would hold the runtime's shutdown — and so the exit
  after `tg serve closed the stream` — until stdin closed.

**Serve request strictness:** `SendRequest`, its nested `media::SendFile`, `ReactRequest` and
`DeleteChatRequest` — the requests that change a chat — are the only serve request structs carrying
`#[serde(deny_unknown_fields)]`. The others are deliberately open: `WhoamiRequest{}` backs
the container's `HealthCmd` (`tg whoami`, `HealthStartupTimeout=2m`), so tightening it risks
the health gate for no benefit, and the remaining structs have caller sets that were never
audited. `dispatch_other_commands_still_ignore_unknown_args` pins the decision so a later
blanket change has to be deliberate.
