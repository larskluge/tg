# Chats

What a chat row says and how a one-to-one chat is deleted. The rules every change keeps are in
[AGENTS.md](../../AGENTS.md); this file is how it works as built.

**A channel is a supergroup with `is_channel` (`is_channel` on every chat row, since 0.13.0):**
TDLib has no chat type of its own for a channel. `chat.type` is `chatTypeSupergroup
{ supergroup_id, is_channel }` for a channel and for every other supergroup, and the flag is the
whole difference ("True, if the supergroup is a channel"); a forum and a broadcast group are
flags on the `supergroup` object, not on the chat type. Read from the `td_api.tl` tdlib-rs 1.3.0
pins, not measured: no test and no agent lists a real account's chats.

- **`chat_type_kind` is the one place the flag is read.** It yields `ChatTypeKind::Channel`, and
  `ChatSnapshot::to_chat_info` turns that into `is_channel` on `ChatInfo`, the row `chats`,
  `groups` and `unread` share, in `--json` output and in a `tg serve` result alike.
- **Always a boolean.** `is_bot` is `Option<bool>` because it takes a `getUser` that can fail;
  `is_channel` sits on the `chat.type` that `fetch_chat_snapshot` already holds, so it costs no
  request and is never unknown. Its `#[serde(default)]` is for `tg`'s own CLI reading rows from a
  `tg serve` older than 0.13.0: a missing key reads `false`.
- **`groups` lists channels, as it always has** (`is_group_chat`: basic group, supergroup,
  channel), and `chats` leaves them out (`is_private_chat`). The row is a contract with Mycelium
  (its spec: `docs/specs/2026-10-12-channel-kind-design.md` in mycelium), which discovers
  Telegram's channels through `groups` and labels a row `channel` when the key is `true`, `group`
  when it is `false` or absent. Taking channels out of `groups`, or giving them a command of
  their own, would end that discovery. There is no filter flag, and the table and plain-text
  output do not show the key.

**Deleting a private chat (`delete_chat`, since 0.17.0):** for Tavola's Delete
(`docs/specs/2026-10-22-delete-conversation-design.md` in tavola, through Postmaster's
`delete_chat`). The contract is in `README.md` → *`delete_chat`*. Read from TDLib's source at
`11e254af` (1.8.61; `td/telegram/…`), not measured: **no test and no agent ever deletes a real
chat.**

- **`deleteChatHistory` has no upper bound of its own.** `MessagesManager::delete_dialog_history`
  (`MessagesManager.cpp:8308`) sends `messages.deleteHistory` (`MessageQueryManager.cpp:1322`)
  with `max_id` = the chat's `last_new_message_id`, the newest message TDLib knows, and while it
  knows none it sends 0 (a "TODO get dialog from the server" sits there), which bounds nothing,
  and the request is in TDLib's binlog until Telegram answers, so one sent with no bound while
  Telegram is out of reach would also delete what came in until it went out. So `delete_chat`
  reads `getChat`; when `last_message` is null (also the moment after a chat is loaded from the
  database) it reads the history from its end (8 s at most), which makes TDLib load it, and reads
  the chat again; and it asks for the delete only while the chat shows a last message no later
  than `through`. A chat still without one is answered `deleted` with no call when it looks
  deleted (`ChatPresence::looks_deleted`, read after a history read from the end, as `sync`'s
  check reads it), and is an error otherwise. A message Telegram holds that TDLib has not
  received yet stays, and keeps the chat. One TDLib receives between the last read and the call,
  a local round trip under the client lock, is deleted with the rest: there is no
  `deleteChatHistory` up to a message id. (`deleteChatMessagesByDate` takes a date range, but
  leaves the chat in its lists and spares the last 30 seconds' messages.)
- **`revoke: false` is never upgraded.** TDLib turns `revoke: true` into false when the chat
  cannot be deleted for everyone, never the other way; with false it refuses only a chat that
  cannot be deleted even for oneself (400 "Can't delete history of … only for self").
- **The state it leaves is a chat deleted in a Telegram app's.** `delete_all_dialog_messages`
  (`:8585`) clears `last_message` (`updateChatLastMessage` with none) and sets the order to
  `DEFAULT_ORDER`, which takes the chat out of every list, then sends `updateDeleteMessages`
  with `is_permanent: true`, `from_cache: false` for the messages it held in memory (not those
  only in its database). So `subscribe` rings `chat_last_message` with `message_id: null` and
  `messages_deleted`, and `sync --report-deleted` answers `{"chat_deleted": true}`: in no list,
  no last message, and the history from the end empty (`delete_dialog_history` sets
  `max_unavailable_message_id` to the last message it deleted up to, which hides everything up
  to it). Mycelium's Telegram adapter marks the messages of `messages_deleted` deleted at once,
  ignores the null `chat_last_message`, and records the chat deleted (`chats.deleted_at`, every
  message deleted) when a full pull's `chat_deleted` still holds `deletion_confirm_after` (5
  minutes) later: 5 to 10 minutes after the delete, with its 5-minute pull.
- **A repeat answers `deleted` without a call.** The chat is still known (`getChat` answers),
  in no list, with no last message and no history: deleted, by the request whose answer was lost
  or in a Telegram app. (`delete_dialog_history` would also answer ok, finding the history
  cleared already, "nothing to do", but only while `last_new_message_id` is known.)
- **Telegram's answer is awaited for 20 s** (`DELETE_CHAT_TIMEOUT_SECS`), because `tg serve`
  holds its client lock meanwhile. TDLib clears the chat on its side before it asks Telegram and,
  with the message database on (as `tg` runs it), logs the request to the binlog and erases it
  only once Telegram answered (`delete_dialog_history_on_server`), so a delete that runs out of
  time is answered as not confirmed, never as failed, and goes through later.
- **`not_found` is TDLib's 400 "Chat not found"**, what `getChat` answers for a valid id it has
  never seen (`load_dialog`, `DialogManager::check_dialog_access`): a chat Telegram may well hold,
  but that this session never loaded. "Invalid chat identifier specified" stays an error.
