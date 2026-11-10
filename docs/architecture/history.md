# Reading history and sync

How `tg` pages a chat's history (`messages`, `tg sync`) and tells a deleted chat from a quiet one.
The rules every change keeps are in [AGENTS.md](../../AGENTS.md); this file is how it works as
built.

**TDLib `getChatHistory` quirk:** May return fewer messages than `limit` on the first call while syncing from the server. Always use a retry+pagination loop: retry on empty responses (up to 5×), and page using the oldest returned message ID as the next `from_message_id`.

**TDLib `getChatHistory` offsets, and walking forward (`tg sync --oldest-first`, since 0.9.0):**
Read from TDLib's source (`OrderedMessages::get_history` in `td/telegram/OrderedMessage.cpp`,
`MessagesManager::get_dialog_history`/`get_history_impl`), because the `td_api.tl` one-liner
("a negative number … to get additionally -offset newer messages") is looser than the code:

- The anchor is the newest message with id `<= from_message_id`. `offset = 0` on an anchor
  that IS `from_message_id` starts strictly OLDER than it — which is why the newest-first
  walker can page with the last id it saw and still dedups defensively.
- `offset = -k` on an existing `from_message_id` spends one of the `k` steps on the anchor
  itself: `k - 1` strictly newer messages come back, then the anchor, then older ones. With a
  deleted `from_message_id` all `k` are strictly newer. So `offset = -1` on an existing message
  returns nothing newer at all — never use it to probe for newer messages.
- Walking off the chat's newest message shrinks the answer (`limit += offset`) instead of
  failing; a local gap makes TDLib load from its database or the server and ask again, and the
  iterator stops at gaps, so the newer messages in an answer are always the run immediately
  after the anchor. That contiguity is what makes "advance the cursor to the newest id
  returned" skip nothing.
- `from_message_id = 0` means "from the last message". To start at a chat's FIRST message,
  anchor at `1 << 20` (server message id 1): TDLib turns it into `messages.getHistory` with
  `offset_id = 1`, Telegram's idiom for reading from the start.

`collect_messages_forward` therefore asks `(from = cursor, offset = -99, limit = 100)` every
time, keeps ids `> cursor`, sorts ascending and stops at `limit`. An EMPTY answer means TDLib is
still loading (retry, like the newest-first walker); a non-empty answer with nothing newer than
the cursor means it answered from the anchor and found nothing after it — caught up, stop at
once. Conflating the two costs every idle chat 1.5 s of retry sleeps per sync, and resetting
the retry count on such an answer loops forever. Stopping early is never lossy: the caller's
cursor only moves past messages it was given. Tested against `WindowedHistory`, a fake that
answers with this window arithmetic (pinned by `windowed_history_models_tdlib_offsets`) rather
than scripted batches.

**TDLib `getChatMessageByDate` direction:** It returns the last message sent **no later than** the given date — the returned message's date is always `<= date` — and a **404** when the chat has no such message. It does not find the first message *after* a date. To turn a `--since-utc` cutoff into a fetch boundary, probe at `cutoff - 1` and use the returned message's `id + 1` as an exclusive lower bound (`boundary_probe_date` / `boundary_from_probe` in `client.rs`). Reading it as "at or after the date" makes the lookup silently never match.

**A deleted private chat (`tg sync --report-deleted`, since 0.10.0):** TDLib has no "chat
deleted" field, and none of the updates `subscribe` forwards says a chat was deleted, so the
signal is read off the chat's state.
Read from TDLib's source (`MessagesManager::update_dialog_pos`, `get_chat_positions_object`,
`set_dialog_is_empty`, `on_get_history`):

- **`chat.chat_lists`, never `chat.positions`.** `positions` reports a list only while the
  chat lies inside the part of that list loaded so far (`list_last_dialog_date_`), so every
  quiet chat has empty `positions` until something loads the whole list. `chat_lists` is
  membership (`d->dialog_list_ids`), independent of loading: empty exactly when TDLib's
  order for the chat is `DEFAULT_ORDER`.
- A chat whose messages are all gone gets that order: deleting its last message makes TDLib
  re-read the history from the server, and an empty answer from the end marks the chat
  `is_empty`, which drops it from every list and clears `last_message`. So a deleted private
  chat reads: in no chat list, no last message, empty history.
- **The same state appears transiently** for a chat just loaded from the database whose last
  message is not loaded yet ("no known messages in the chat, just leave it where it is" keeps
  the in-memory default order until `load_last_dialog_message` answers). That is why
  `chat_is_deleted` reads `getChat`, then the history from the end (`getChatHistory` loads
  from the database and then the server before it answers empty), then `getChat` again, and
  reports a deletion only if the state held throughout. An error at any step is an error.
- A chat TDLib has never seen answers `getChat` with "Chat not found" — an error, not a
  deletion. That is what a session created after the deletion sees, so the signal only
  works in a session that synced the chat before it was deleted, like `tg serve`'s.
- Measured 2026-09-28 against `tg serve` on outpost, every chat Mycelium syncs at its own
  cursor: 2 of 73 DMs reported `chat_deleted` — the two a local session created later did
  not know at all — and the other 71 DMs and 6 groups read as ordinary chats.
- Only private chats are reported: a basic group or supergroup also leaves every list when
  you leave it, but leaving a group is not deleting a chat.
