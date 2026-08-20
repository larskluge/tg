# Message formatting (`--parse-mode` / `args.parse_mode`)

## Problem

`tg`'s TDLib send path hardcoded an empty entity list:

```rust
InputMessageText { text: FormattedText { text: text.to_string(), entities: vec![] }, .. }
```

TDLib does **not** auto-detect bold/italic/code server-side — its entity auto-detection covers
only link-ish entities — so a body written in markup was delivered with the markup visible.
The callers that matter here (an approval pipeline dispatching agent-written bodies through the
`tg serve` socket) write markdown, and their recipients saw literal asterisks.

Two smaller faults sat next to it:

- `send`'s socket args were deserialized with no `deny_unknown_fields`, so an unsupported key
  was dropped and `tg` still answered `ok: true`. For a recipient or identity field that is a
  message delivered to the wrong place with no signal anywhere.
- The bot HTTP path (`tg send --as @bot`) built its own payload with only `chat_id` and `text`,
  so adding a flag without touching it would have created a fresh silent drop.

## Contract

- Arg name `parse_mode` on the socket, `--parse-mode` on the CLI.
- Accepted values: exactly `HTML` and `MarkdownV2`. Case-sensitive, no trimming, no aliases.
- Absent means today's behaviour, **byte-for-byte**: the `None` branch builds the same
  `FormattedText { text, entities: vec![] }` in the same struct, producing the same TDLib
  request JSON; the bot payload omits the key entirely rather than sending `null`.
- Anything else is refused in-band with ``invalid parse_mode '<value>'. Expected `HTML` or
  `MarkdownV2` `` and **nothing is sent**.

Never falling back to plain text is the point. A fallback would deliver a body a human
approved as formatted, wearing visible `<b>` tags, with `ok: true` — the exact failure this
change exists to remove, moved one layer down. A refusal is loud and retryable.

## Shape

| Concern | Where |
| --- | --- |
| The protocol value | `src/parse_mode.rs` — `ParseMode::{parse, as_str}`, no `tdlib_rs` dependency |
| TDLib mapping | `src/client.rs` — `tdlib_parse_mode()`, `unwrap_formatted_text()` |
| Trait | `TelegramClient::send_message(&self, chat_id, text, parse_mode: Option<ParseMode>)` |
| Wire mirror + validation | `src/commands/send.rs` — `SendRequest.parse_mode: Option<String>`, validated in `handle` |
| CLI | `src/cli.rs` — `SendArgs.parse_mode: Option<String>` |
| Bot HTTP | `src/bot_api.rs` — `send_message_payload()` |

`ParseMode` is a top-level module rather than a neighbour of `DownloadOptions` in `client.rs`
because it is a *protocol* value: it appears on the socket wire, on the CLI and in the Telegram
Bot HTTP payload, and is consumed by four modules. A protocol type imported by the HTTP path
out of the TDLib client module would be the wrong home. Keeping it free of `tdlib_rs` also
makes it unit-testable in isolation.

`SendRequest.parse_mode` stays `Option<String>` so the struct remains a transparent wire mirror,
so `From<SendArgs>` stays infallible, and so the error text stays ours rather than serde's.
The CLI flag is `Option<String>` + the shared validator rather than a clap `ValueEnum` for the
same reason: a `ValueEnum` would need `#[value(name = "HTML")]` overrides to match the wire
literals and would become a second copy of the accepted set that can drift, while the handler
validator stays mandatory regardless (socket callers never touch clap). One list, one error
text on every path.

## Ordering, and why it is load-bearing

**In `send::handle`, `parse_mode` is validated before the target ladder.** The ladder issues
TDLib contact and public-chat searches; a malformed request must not cost a round trip. It is
also the only ordering under which a probe can tell an upgraded daemon from an un-upgraded one
(see *Deploy verification*). Pinned by `handle_validates_parse_mode_before_resolving_target`
and `handle_validates_parse_mode_before_contact_lookup`.

**In `TdLibClient::send_message`, parsing happens before `create_private_chat` and before
`functions::send_message`.** `parseTextEntities` is a TDLib static request: local computation,
no network call, no authorization needed, ~17 µs. So a bad body returns with zero residue — no
message, no draft, no opened chat — and no code path reaches the transmit call with an unparsed
body, because `?` returns first. "Silently unformatted" is unreachable by construction.

The reachable failure is *silently mis-formatted*, which is a caller-side property of
MarkdownV2 and is documented in README rather than defended in code — see below.

## Two guards the naive version misses

**A parse failure is not `TgError::TdLib`.** The send call five lines down maps to that same
variant, so reusing it makes a permanent markup fault indistinguishable from a transient
network or flood error. Worse, an approval UI that offers "retry" would re-dispatch the
byte-identical body forever. The parse error is its own message and says so:

```
parse_mode HTML: <TDLib's message, which names the offending byte offset> — nothing was sent;
the markup must be fixed and re-proposed, retrying the same body will fail identically
```

**A non-empty body can parse to nothing.** `<b></b>` under HTML and `**` under MarkdownV2 both
return `Ok` with an empty string — an agent emitting an empty-bold placeholder, or an HTML
converter emitting `<b></b>` for an empty heading, produces exactly this. `resolve_message`
already guarantees a non-empty body for every plain-text send, so the invariant is preserved
here rather than handed to TDLib as an empty message that fails opaquely after a chat has been
opened:

```
parse_mode MarkdownV2: message body is empty after parsing markup; nothing was sent
```

An input that was *already* empty is left alone, so that case keeps behaving as it does today.

## `deny_unknown_fields`, on `SendRequest` only

It protects a **new server from an old caller**, never a new caller from an old server: an
un-upgraded `tg` has neither the field nor the strictness, so a caller that ships `parse_mode`
first gets `ok: true` and a recipient who sees literal `<b>` tags. The wire protocol has no
version handshake. Deploy sequencing is the only protection for the rollout — see below.

Scope is deliberate. The other ten request structs stay open: `WhoamiRequest{}` backs the
container's `HealthCmd` (`tg whoami`, `HealthStartupTimeout=2m`), so tightening it risks the
health gate for zero benefit, and the rest have caller sets that were never audited.
`dispatch_other_commands_still_ignore_unknown_args` pins that so a later blanket change is a
conscious act.

## Testing

31 new tests. What they cover, and what they cannot:

- `ParseMode::parse` — both literals, wrong case, unknown values (including `Markdown`, so the
  legacy laxer mode stays unreachable), empty and whitespace, and the exact error string, which
  is a wire contract a caller may match on.
- `tdlib_parse_mode` — that `Html` is the unit variant and that MarkdownV2 is `version: 2`. That
  second one catches the highest-consequence silent typo in the change.
- `MockClient` now records `(chat_id, text, parse_mode)` instead of discarding the text, so the
  handler tests can assert the mode reached the client — and, for a refused mode, that the
  recording is **empty**.
- Socket dispatch — a good mode succeeds, a bad mode and an unknown arg both fail in-band with
  the id echoed.
- Bot payload — the key is *absent*, not `null`, when no mode is set.

**Not coverable here:** malformed-markup errors and UTF-16 offset correctness, because
`parse_text_entities` in tdlib-rs 1.3.0 routes through `send_request` → observer → a live
receive loop rather than a synchronous `td_execute`. Both were verified out-of-band against
real libtdjson 1.8.61 and the findings are recorded in AGENTS.md. The structural reason the
offset class of bug is absent is that `tg` computes no offsets and escapes nothing: it hands
the raw string to TDLib.

## Deploy

`outpost` runs `telegram.service`, a podman quadlet on `ghcr.io/larskluge/tg:latest` with
`AutoUpdate=registry`, socket shared via the `tg-ipc` volume. The daemon is live and approvals,
hermes and mycelium talk to it.

**Sequencing is mandatory:** roll `tg` and verify the digest on `outpost` *before* any caller
starts sending `parse_mode`. Shipping the caller first yields `ok: true` and literal tags.

### Verification probes

Probe A (`{message}`) and probe B (`{message, parse_mode}`) do **not** discriminate: both return
the same recipient error before and after the change. The discriminating probes are:

- **C** — `{"message":"x","parse_mode":"markdown"}` with no recipient. An upgraded daemon
  answers ``invalid parse_mode 'markdown'. Expected `HTML` or `MarkdownV2` ``; an un-upgraded
  one answers the recipient error. This only works because validation precedes target
  resolution (`handle_validates_parse_mode_before_resolving_target`) — move that check below
  the ladder and probe C silently degrades into a pass against an un-upgraded daemon.
- **D** — `{"message":"x","id":1,"bogus":true}`. An upgraded daemon answers
  ``invalid args: unknown field `bogus`, expected one of ...``; an un-upgraded one ignores it.

Plus `tg --version` reporting 0.4.6, and a live formatted send to Saved Messages with emoji and
umlauts on both sides of a formatted span, read back and eyeballed. Failure signature: bold or
italic starting one or two characters off.

### Rollback

The pre-roll image is still pullable from GHCR by digest, so the fast path needs no GitHub:

```bash
systemctl --user stop podman-auto-update.timer   # REQUIRED — see below
podman pull ghcr.io/larskluge/tg@sha256:1185d2e994237f4f68654b8ec2f53de4843d8f4e956f9d32661746af0adc4fbc
podman tag ghcr.io/larskluge/tg@sha256:1185d2e994237f4f68654b8ec2f53de4843d8f4e956f9d32661746af0adc4fbc ghcr.io/larskluge/tg:latest
systemctl --user restart telegram.service
```

Digests: `1185d2e9…` is the OCI index, `cbb44a9a…` the amd64 manifest. The *local* copy is
pruned by the quadlet's `ExecStartPost=podman image prune -f`, but the registry copy is not.

Stopping the timer is not optional: with `AutoUpdate=registry` and the daily
`podman-auto-update.timer` enabled, the next run sees registry `latest` ≠ local and rolls the
bad image forward again, silently undoing the rollback. Either stop the timer or land the
revert on `main` first.

`podman auto-update --rollback` defaults to true and runs before the prune, so an image that
*fails to start* self-heals within the same run — but that covers only crash-on-start, not
"starts fine, formats wrongly", which is precisely the failure this change can produce.

Reverting through CI instead takes ~8 minutes (measured on `main`) plus an auto-update cycle.
