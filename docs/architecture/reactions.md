# Reactions

Listing, ringing and sending reactions, and `tg serve`'s poll of group reactions. The rules every
change keeps are in [AGENTS.md](../../AGENTS.md); this file is how it works as built.

**Reactions (since 0.12.0):** a listed message carries `reactions`, `subscribe` rings
`message_reactions`, `message` re-reads one message, `react` adds or takes back. The shapes are
a contract with Mycelium (its `message_reactions` projector) and Postmaster (`send_reaction`),
documented in `README.md` → *Reactions*, *`react`*, *One message*. All of the following was read
from TDLib's source at the commit tdlib-rs 1.3.0 pins (`11e254af`, 1.8.61) and from Telegram's API
docs, not measured: **no test and no agent ever sends a reaction to a real chat.**

- **Measured read-only on 2026-10-06** (a local session, listing only, counts only): 2,669
  messages of 37 chats, 389 with reactions in 438 entries; no emoji with U+FE0F in any of them;
  the account never among `recent_senders`; in all 232 entries of one-to-one chats
  `count == chosen + recent_senders.len()`; never more than three senders; 32 entries with an
  uncredited rest. `getMessageAvailableReactions` offered 73 emoji for a group message and for a
  private one, none with the selector, and `telegram_form` found the bare heart for a keyboard's.
  `getMessage` for an id the chat does not hold answers `Not Found`. On outpost's `tg serve` the
  same day, after the roll to 0.12.0: 1,956 messages of 35 chats, 274 with reactions in 280
  entries, and ONE emoji with U+FE0F — U+1F441 U+FE0F in a group, which is not a reaction
  Telegram offers, so a custom emoji listed as its sticker spells it. Telegram's own reactions
  carry no selector; what `tg` lists can.
- **`reactions::listed_reactions` is the one reader of `interaction_info.reactions`**, as
  `reply_in_chat` is of `reply_to`. It drops paid reactions, lists a custom emoji under the
  emoji `getCustomEmojiStickers` names for it (one call per batch of messages, cached by TDLib,
  5 s at most; unnamed ones are left out with a line on stderr), merges two TDLib reactions that
  are one emoji once variation selectors are set aside, and keeps the account out of the senders.
- **`used_sender_id` is how the account is recognised among `recent_sender_ids`.** TDLib sets it
  exactly when the reaction is chosen AND the account's own sender is among the recent choosers it
  returns (`MessageReaction::get_message_reaction_object`): always in a private chat, where it
  builds the list itself (the account if chosen, then the peer if the count leaves room), and in
  a group only when Telegram named the account among the recent three. So "drop the sender equal
  to `used_sender_id`" is exact, and `count - chosen - recent_senders.len()` is the uncredited
  rest. `recent_senders[].id` is `null` for a chat sender, as `sender_id` is on a message.
- **TDLib compares a reaction to the offered ones byte for byte.** `ReactionType` keeps the string
  as given and `addMessageReaction` answers 400 `The reaction isn't available for the message`
  unless `ChatReactions::is_allowed_reaction_type` finds it. Telegram's forms carry no U+FE0F (its
  heart is U+2764), a keyboard's do. `react` therefore reads `getMessageAvailableReactions` first
  and sends the offered emoji that equals the asked one without variation selectors
  (`telegram_form`); none found, it sends the emoji as given so the refusal is Telegram's own.
  The three lists of `availableReactions` together hold every offered reaction
  (`get_sorted_available_reactions` ends in `CHECK(all_available_reaction_types.empty())`), and
  the request is answered from TDLib's memory. Do not replace this with "strip U+FE0F and retry
  on that error text": it works today only because Telegram happens to offer no emoji with the
  selector.
- **`addMessageReaction` changes TDLib's own copy first.** `set_message_reactions` sends
  `updateMessageInteractionInfo` and only then `messages.sendReaction`; the request's answer is
  Telegram's. On an error TDLib queues a reload of the message's reactions. So: a `react` through
  `tg` rings a `message_reactions` frame by itself; a read-back right after an ERROR may still
  show the reaction (which is why `react` reads back only after success); and a request that
  times out says nothing about the outcome. Reactions are not in TDLib's binlog ("TODO … log
  event"). An account without Premium has one reaction per message, and adding the same one again
  changes nothing on Telegram.
- **TDLib answers a repeat from its own copy, so `tg` keeps what Telegram has not answered (since
  0.15.1, tavola-fkp.6).** A second `addMessageReaction` for a reaction the copy already shows is
  answered ok at once, without asking Telegram (`MessagesManager::add_message_reaction`:
  `add_my_reaction` returns false for one already chosen and not big, and the promise is set), and
  `removeMessageReaction` likewise for one it no longer shows; `react`'s read-back reads that same
  copy. So a request that stopped waiting after 10 s did not end its change: `ReactionChanges`
  runs every reaction call on a task of its own and keeps the change, per `(chat_id, message_id)`,
  until Telegram answers (`Settled` takes it off however the task ends). `react::handle` asks
  `earlier_reaction_change` before anything else and waits up to 10 s more: still unanswered, it
  fails and asks nothing; refused, a repeat of that change (`ReactionChange::repeats`: the same
  emoji in either spelling, any taking back for a taking back of everything) gets the refusal,
  since TDLib's copy shows the refused change until its reload lands; otherwise it goes ahead. The
  mock models TDLib the same way (its copy first, a silent Telegram on `telegram_answers`), so the
  tests drive the real `ReactionChanges`. What it cannot cover: a process that stops forgets what
  it asked, and a new one's copy may show a change Telegram never got.
- **`removeMessageReaction` takes back exactly the type it is given** and answers ok, having done
  nothing, for one the account has not chosen. What `tg` lists as one emoji can be a custom emoji,
  so `remove_message_reaction` reads the message and takes back `reactions::chosen_types`.
- **`message_reactions` rings on every `updateMessageInteractionInfo`**, with or without an
  `interaction_info`: the last reaction taken back leaves none, and that update is the only sign
  of it. The same update carries a channel post's view and forward counters and a discussion
  thread's reply count, and an update holds only the new state, so telling those apart would
  take a cache of what each message had. The consumer compares instead.
- **Which reaction changes reach TDLib on their own** — what Telegram pushes, whether or not a
  chat is open:
  - *One-to-one chats:* every change. TDLib never polls there
    (`need_poll_dialog_message_reactions` is `false` for `DialogType::User`).
  - *Basic groups and supergroups:* a reaction to the account's OWN message (Telegram's API docs:
    "Message authors will receive an updateMessageReactions update when a user reacts to their
    message", subject to the account's reaction-notification setting), and the account's own
    reaction made through `tg`. Whether the account's own reaction made on another device arrives
    is not established. A reaction between other people is not pushed: Telegram's docs tell apps
    to "short-poll reactions for visible messages (that weren't sent by the user) once every 15-30
    seconds", and TDLib does that only for messages reported with `viewMessages` while the chat's
    `open_count > 0`. Hence the poll below.
  - *Reading again does not help.* `getChatHistory` returns what `ordered_messages` holds without
    asking Telegram (`get_dialog_history`), and `getMessage` asks only for a message TDLib does
    not have. So `sync --reconcile-days` and `message` answer with TDLib's copy, and a consumer's
    reconcile catches missed doorbells, not reactions TDLib never received.

**The reaction poll (`tg serve`, since 0.15.0; `reaction_poll.rs`).** `tg serve` does what a
Telegram app does for the messages on its screen: every group whose last message is at most seven
days old is opened (`openChat`), its newest messages of those seven days (at most 100, read from
TDLib's own copy with `only_local`, never a live location) are reported with `viewMessages`, and it
is closed (`closeChat`). TDLib then asks Telegram for their reactions, and every change rings as an
ordinary `message_reactions` frame: no consumer changes anything. All of the following is read from
TDLib's source at `11e254af` (1.8.61; `td/telegram/…`), so **check it again when tdlib-rs moves to
another TDLib**:

- **The source is the whole risk, and it is `messageSourceNotification`, `force_read: false`.**
  `view_messages` (`MessagesManager.cpp:15959`) turns `Auto` into `DialogHistory` while the chat is
  open (`:15965`), and only a history source or `force_read` sets `need_read` (`:15974`). Everything
  that marks something read waits on it: the unread mentions and voice/video contents
  (`read_message_contents_on_server`, `:16158`), the unread reactions to the account's messages
  (`read_message_reactions_on_server`, `:16164`) and the inbox (`read_dialog_inbox`, `:16328`, after
  `if (!need_read) return` at `:16261`). `messageSourceOther` would do too, but it also queues
  `messages.getMessagesViews` for a message with views (`need_update_view_count`, `:15975`; without
  the view-counter increment, which only history sets, `:16225`) and marks downloads viewed in the
  download manager (`:15977`). A notification sets neither, so it is the source with the fewest side
  effects that still polls. The default source in an open chat MARKS THE MESSAGES READ: never pass
  `None`.
- **A live location is never reported.** Viewing an active live location in an open chat, from any
  source, sends `readMessageContents` for it (`on_message_live_location_viewed`, `:16153` →
  `:18086`). `is_live_location` leaves out every `messageLocation` with a `live_period`.
- **What polls.** A viewed server message of an open chat goes into `dialog_viewed_messages_`
  (`:16187`) and through `process_viewed_message` (`:9584`), which queues a reaction reload for the
  ones `need_poll_message_reactions` picks (`:6086`): a message TDLib holds reactions for, or one
  last updated under another generation of the chat's available reactions. A message nobody reacted
  to is not polled: Telegram sends a message's first reaction as an edit of it (tdesktop's poll says
  so: "First reaction always edits message"; not read in Telegram's own docs), which reaches TDLib
  with the chat's updates, and `openChat` on a supergroup fetches those (`get_channel_difference`,
  `:16486`). `queue_message_reactions_reload` (`MessageQueryManager.cpp:2467`) sends one
  `messages.getMessagesReactions` per 100 messages at once; the answer's `updateMessageReactions`
  updates the message, and a message the answer leaves out is set to no reactions (`:1008`).
- **`closeChat` right after does not cancel it.** `close_dialog` (`MessagesManager.cpp:16534`)
  drops `dialog_viewed_messages_` (`:16576`) and the repeat timer, but the reload queue
  (`being_reloaded_reactions_`) is `MessageQueryManager`'s and is left alone. The repeat timer would
  not run anyway: it needs the account online (`:16251`, `:9574`, `:9617`), and `tg` never sets
  the `online` option (`OnlineManager.h:52`: offline by default), so the poll also never shows the account
  online. Each poll is exactly one reaction reload per chat.
- **What `openChat` itself costs**, all reads (`open_dialog`, `:16420`): a basic group's full info
  (`repair_chat_participants`) and peer settings; a supergroup's difference, peer settings, full
  info when it is out of date, and its 200 recent members when it has fewer than 195; the pinned
  message and a group call's state when not cached; and the chat goes onto TDLib's local list of
  recently opened chats. Roughly 3 to 5 requests per open, plus the reaction reload. Reporting
  also reloads a fact check, a paid medium or a forwarded story where a message carries one.
  Reading the history with `only_local` waits on no request, though TDLib may preload the
  neighbouring history in the background, as it does for every history read. Listing the chats
  (`getChats` for Main and Archive) loads the lists from Telegram the first time, once per
  session, and holds the client lock meanwhile.
- **The guard.** Each poll reads the chat's read state (`unread_count`, `unread_mention_count`,
  `unread_reaction_count`, `is_marked_as_unread`, `last_read_inbox_message_id`) before
  `openChat` and again after `viewMessages`, before `closeChat`. TDLib changes it synchronously
  inside `view_messages` when it reads anything (and `open_dialog` reads locally only in a chat
  without a last message, `:16440`, which the poll never opens), while what `openChat`'s requests bring back (a read made on another device) lands
  later. A count that falls, the mark that goes or the inbox's read mark moving up (which no new
  message can hide, as one can hide a lower count) stops the poll until `tg serve` restarts
  (`Stop::Dropped`), with one line on stderr; so does a read state that cannot be read
  afterwards (`Stop::Unverified`). A read made on another device inside those few milliseconds
  would stop it too. A chat is closed on every path once opened; one whose `closeChat` failed is
  closed again before the next round and when the poll ends.
- **Tiers.** By the age of a group's last message: at most an hour, every 5 minutes; a day, every
  20; seven days, every 60. A round starts every minute and polls what is due, the most recently
  active first, one chat at a time with 2 s between them; a round never overlaps the next, and an
  interval runs from the round a chat was polled in. Only a basic group or a non-channel supergroup
  of the Main or Archive list that allows reactions is polled; the first round comes a minute after
  start. The poll holds the client lock for one chat's reads and calls at a time (local, a few
  milliseconds), so a send waits at most that; a round reads every listed chat's state, one
  `getChat` each, to find the groups that are due.
- **`TG_REACTION_POLL=off`** turns it off; any value but `on` or `off` refuses to start `tg serve`.
- **Logs are numbers only:** the first round's counts (groups in the window, polled, reported,
  with reactions, and the unread totals before and after), a summary every hour (rounds, polls,
  frames rung within 10 s of a poll, the unread totals now), each distinct error once, and the stop.
- **A reaction found by a poll is dated late by the consumer.** Mycelium dates a reaction it hears
  of by the ring. A ring from a poll comes up to one interval after the reaction was made; the first
  poll after the roll found everything TDLib had missed until then.
- **Measured on outpost, 2026-10-18** (0.15.0 rolled at 11:47 UTC; numbers only). Before: 117
  non-channel groups, 37 of them with a message in the last seven days (3 basic groups, 34
  supergroups; 1 within the hour, 15 within the day), `tg groups`' unread total 109,169 over 57
  groups with unread. The first round polled all 37 in about 75 s: 1,006 messages reported, 67 with
  reactions, and in every polled chat the read state after reporting equalled the one before
  (summed: 106,246 unread messages, 0 mentions, 1 reaction, 0 chats marked unread). The first hour:
  60 rounds, 114 polls, 79 of them re-reading reactions, 6,240 messages reported, 83
  `message_reactions` frames within 10 s of a poll, none failed and the guard never stopped it.
  So about 114 opens of 3 to 5 requests each plus 79 `getMessagesReactions`, some 420 to 650
  requests in the first hour, and about 77 opens (300 to 450 requests) in an hour after it. The
  frames came with the first polls of each message: a 7-minute `tg stream` an hour later carried
  none. Unread over all 117 groups: 109,176 at 12:48 and 109,178 at 13:20, more messages; one
  chat's count fell by 3, which were three weeks-old messages deleted on Telegram that TDLib heard of
  from the difference its `openChat` fetched (Mycelium recorded the three deleted at 11:49:26,
  during the first round): a deletion, not a read. In the 13 groups Mycelium keeps durably, other
  people's reactions to the last seven days' messages read 22 in TDLib and 22 in Mycelium after
  the first hour, and no reaction between other people changed in any polled group between 12:50
  and 13:20 (1,049 messages compared): a new one arriving through the poll is still to be seen.
