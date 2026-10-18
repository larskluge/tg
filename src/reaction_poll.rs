//! `tg serve`'s poll of group reactions (since 0.15.0).
//!
//! Telegram sends a reaction change in a basic group or supergroup on its own only when it
//! concerns the account (a reaction to its own message, or its own reaction). A Telegram app
//! short-polls the messages on its screen for the rest, and TDLib does that poll
//! (`messages.getMessagesReactions`, one request per 100 messages) for the messages it is told
//! are viewed while their chat is open. So the poll opens each recently active group, reports its
//! newest messages as viewed, and closes it again. The changes Telegram answers with then ring as
//! ordinary `message_reactions` frames.
//!
//! **The source is what keeps it from reading anything.** The messages are reported with
//! `messageSourceNotification` and `force_read: false`: TDLib marks messages, mentions and
//! reactions read only for a chat-history source or `force_read`, and the default source in an
//! open chat IS chat history. A live location is never reported, since viewing one marks its
//! content read whatever the source. AGENTS.md, *Reactions*, has the TDLib source this rests on.
//! Every poll also checks it: a chat's read state is read before it is opened and again after its
//! messages are reported, and anything read in between stops the poll for the rest of the session
//! ([`Stop::Dropped`]).

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tdlib_rs::enums::Update;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, broadcast, watch};
use tokio::time::Instant;

use crate::error::{Result, TgError};

/// The variable that switches the poll: unset or `on` polls, `off` does not.
pub const ENV: &str = "TG_REACTION_POLL";

/// How far back a poll looks: a group whose last message is older is not polled, and no message
/// older than this is reported. Mycelium's hourly reconcile compares the same seven days.
pub const WINDOW_SECS: i64 = 7 * 24 * 3600;

/// The most messages reported per chat and poll: the newest ones within [`WINDOW_SECS`]. TDLib
/// asks Telegram for 100 messages' reactions per request.
pub const MAX_MESSAGES: usize = 100;

/// How often a group is polled, by the age of its last message: the first tier whose age it is
/// within. A group with a message in the last hour is where reactions are being made.
pub const TIERS: [(i64, Duration); 3] = [
    (3600, Duration::from_secs(5 * 60)),
    (24 * 3600, Duration::from_secs(20 * 60)),
    (WINDOW_SECS, Duration::from_secs(60 * 60)),
];

/// How many pages of local history one chat's poll reads at most.
pub const MAX_HISTORY_PAGES: usize = 5;

/// Whether the poll runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    On,
    Off,
}

/// Read [`ENV`]'s value. Anything but unset, `on` or `off` is refused, so a typo cannot quietly
/// leave the poll running.
pub fn setting(value: Option<&OsStr>) -> Result<Setting> {
    match value.map(|v| v.to_str()) {
        None => Ok(Setting::On),
        Some(Some("on")) => Ok(Setting::On),
        Some(Some("off")) => Ok(Setting::Off),
        Some(_) => Err(TgError::Other(format!(
            "{ENV} must be on or off; refusing to start tg serve"
        ))),
    }
}

/// How often a group whose last message is `age_secs` old is polled; `None`: not at all.
pub fn interval_for(age_secs: i64) -> Option<Duration> {
    TIERS
        .iter()
        .find(|(max_age, _)| age_secs <= *max_age)
        .map(|(_, every)| *every)
}

/// What TDLib holds of a chat's read state: everything a read would change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadState {
    pub unread_messages: i64,
    pub unread_mentions: i64,
    pub unread_reactions: i64,
    /// The account marked the chat unread by hand; any read of it clears the mark.
    pub marked_unread: bool,
    /// The newest incoming message read. A read moves it up, and no message arriving meanwhile
    /// can hide that, as one can hide a lower count.
    pub last_read_inbox_message_id: i64,
}

impl ReadState {
    /// Something was read between `before` and this: a count fell, the mark went, or the inbox's
    /// read mark moved up.
    pub fn read_since(&self, before: &ReadState) -> bool {
        self.unread_messages < before.unread_messages
            || self.unread_mentions < before.unread_mentions
            || self.unread_reactions < before.unread_reactions
            || (before.marked_unread && !self.marked_unread)
            || self.last_read_inbox_message_id > before.last_read_inbox_message_id
    }
}

impl std::fmt::Display for ReadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unread messages {}, mentions {}, reactions {}, marked unread {}",
            self.unread_messages, self.unread_mentions, self.unread_reactions, self.marked_unread
        )
    }
}

/// Unread counts summed over chats, for the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub messages: i64,
    pub mentions: i64,
    pub reactions: i64,
    pub marked_unread: i64,
}

impl Totals {
    fn add(&mut self, state: &ReadState) {
        self.messages += state.unread_messages;
        self.mentions += state.unread_mentions;
        self.reactions += state.unread_reactions;
        self.marked_unread += i64::from(state.marked_unread);
    }
}

impl std::fmt::Display for Totals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unread messages {}, mentions {}, reactions {}; {} marked unread",
            self.messages, self.mentions, self.reactions, self.marked_unread
        )
    }
}

/// What the poll reads of a chat before deciding to poll it, from TDLib's own copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatState {
    /// A basic group or a supergroup that is no channel. Telegram sends every change in a
    /// one-to-one chat (TDLib never polls one), and a channel is no conversation.
    pub is_group: bool,
    /// The chat allows reactions. TDLib polls none where it does not.
    pub reactions_allowed: bool,
    /// When the chat's last message was sent, in Unix seconds.
    pub last_message_date: Option<i64>,
    pub read: ReadState,
}

impl ChatState {
    /// How often this chat is polled at `now_unix`; `None`: not at all.
    pub fn interval(&self, now_unix: i64) -> Option<Duration> {
        if !self.is_group || !self.reactions_allowed {
            return None;
        }
        interval_for(now_unix - self.last_message_date?)
    }
}

/// One message of a chat's recent history, as far as the poll needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentMessage {
    pub id: i64,
    /// When it was sent, in Unix seconds.
    pub date: i64,
    /// TDLib holds reactions for it: TDLib's poll re-reads these.
    pub has_reactions: bool,
    /// A live location. Viewing one, from any source, makes TDLib mark its content read.
    pub is_live_location: bool,
}

/// The messages a poll reports of `messages` (newest first): those since `since` that are no
/// live location, at most [`MAX_MESSAGES`].
pub fn messages_to_report(messages: &[RecentMessage], since: i64) -> Vec<i64> {
    messages
        .iter()
        .filter(|m| m.date >= since && !m.is_live_location)
        .take(MAX_MESSAGES)
        .map(|m| m.id)
        .collect()
}

/// What the poll asks of TDLib. Every call but the three that report messages reads TDLib's own
/// copy; listing the chats may first load the lists from Telegram, once per session.
#[async_trait]
pub trait PollSource: Send + Sync {
    /// The chats of the Main and the Archive list.
    async fn poll_chat_ids(&self) -> Result<Vec<i64>>;
    async fn poll_chat_state(&self, chat_id: i64) -> Result<ChatState>;
    /// The chat's newest messages in TDLib's copy, newest first, none sent before `since`, at
    /// most `limit`. Waits on no request: TDLib may go on to preload neighbouring history in the
    /// background, as it does for any history read.
    async fn local_recent_messages(
        &self,
        chat_id: i64,
        since: i64,
        limit: usize,
    ) -> Result<Vec<RecentMessage>>;
    async fn poll_open_chat(&self, chat_id: i64) -> Result<()>;
    /// `viewMessages` with `messageSourceNotification` and `force_read: false`.
    async fn report_viewed(&self, chat_id: i64, message_ids: &[i64]) -> Result<()>;
    async fn poll_close_chat(&self, chat_id: i64) -> Result<()>;
}

/// One page of a chat's history in TDLib's copy: the messages older than `from_message_id` (the
/// newest when 0), newest first, at most `limit`. TDLib's `getChatHistory` with offset 0 starts
/// strictly below an anchor it holds (`OrderedMessages::get_history`).
#[async_trait]
pub trait LocalHistory: Send + Sync {
    async fn local_page(
        &self,
        chat_id: i64,
        from_message_id: i64,
        limit: usize,
    ) -> Result<Vec<RecentMessage>>;
}

/// Read a chat's newest messages since `since`, at most `limit`, page by page from its newest
/// message. An empty page ends it: TDLib's copy has nothing further.
pub async fn collect_recent<H: LocalHistory + ?Sized>(
    history: &H,
    chat_id: i64,
    since: i64,
    limit: usize,
) -> Result<Vec<RecentMessage>> {
    let mut out: Vec<RecentMessage> = Vec::new();
    let mut from = 0;
    for _ in 0..MAX_HISTORY_PAGES {
        if out.len() >= limit {
            break;
        }
        let anchor = from;
        let page = history
            .local_page(chat_id, anchor, limit - out.len())
            .await?;
        let mut took = false;
        for message in page.into_iter().filter(|m| anchor == 0 || m.id < anchor) {
            took = true;
            if message.date < since {
                return Ok(out);
            }
            from = if from == 0 {
                message.id
            } else {
                from.min(message.id)
            };
            out.push(message);
            if out.len() >= limit {
                return Ok(out);
            }
        }
        if !took {
            break;
        }
    }
    Ok(out)
}

/// What one chat's poll did once its messages were reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatPoll {
    /// Messages reported viewed.
    pub reported: usize,
    /// Of them, those TDLib held reactions for.
    pub with_reactions: usize,
    /// The chat's read state before it was opened, and after its messages were reported but
    /// before it was closed: only what opening it and reporting them did can lie between. What
    /// `openChat`'s requests bring back (a read made on another device) lands later.
    pub before: ReadState,
    pub after: ReadState,
}

impl ChatPoll {
    pub fn read_something(&self) -> bool {
        self.after.read_since(&self.before)
    }
}

/// What one chat's poll came to.
#[derive(Debug)]
pub enum Outcome {
    /// Nothing was opened: the chat has no message to report.
    NothingToReport,
    /// Nothing was opened: reading the chat failed first.
    Failed(TgError),
    /// The chat was opened and its read state read again afterwards. `error` is what reporting
    /// or opening answered when it failed; `closed` is whether `closeChat` succeeded.
    Polled {
        poll: ChatPoll,
        error: Option<TgError>,
        closed: bool,
    },
    /// The chat was opened, and its read state could not be read afterwards: nothing shows
    /// whether something was read.
    Unverified(TgError),
}

/// Poll one chat: open it, report its newest messages since the window's start as viewed from a
/// notification, read its read state again, close it. Once it is opened it is closed, whatever
/// the calls between answered (closing a chat TDLib did not open does nothing).
pub async fn poll_chat<S: PollSource + ?Sized>(source: &S, chat_id: i64, now_unix: i64) -> Outcome {
    let since = now_unix - WINDOW_SECS;
    let messages = match source
        .local_recent_messages(chat_id, since, MAX_MESSAGES)
        .await
    {
        Ok(messages) => messages,
        Err(e) => return Outcome::Failed(e),
    };
    let ids = messages_to_report(&messages, since);
    if ids.is_empty() {
        return Outcome::NothingToReport;
    }
    let with_reactions = messages
        .iter()
        .filter(|m| m.has_reactions && ids.contains(&m.id))
        .count();
    let before = match source.poll_chat_state(chat_id).await {
        Ok(state) => state.read,
        Err(e) => return Outcome::Failed(e),
    };

    let error = match source.poll_open_chat(chat_id).await {
        Ok(()) => source.report_viewed(chat_id, &ids).await.err(),
        Err(e) => Some(e),
    };
    let after = source.poll_chat_state(chat_id).await;
    let closed = source.poll_close_chat(chat_id).await.is_ok();
    match after {
        Ok(after) => Outcome::Polled {
            poll: ChatPoll {
                reported: ids.len(),
                with_reactions,
                before,
                after: after.read,
            },
            error,
            closed,
        },
        Err(e) => Outcome::Unverified(e),
    }
}

/// The poll's own timings, apart from the tiers.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// From serve's start to the first round: what the start itself makes TDLib do comes first.
    pub start_delay: Duration,
    /// From one round's start to the next.
    pub tick: Duration,
    /// Between two chats of one round, so a round's requests are spread out.
    pub pace: Duration,
    /// How long after a chat's poll a `message_reactions` frame from it counts as the poll's.
    pub ring_window: Duration,
    /// How often the summary line is written.
    pub report_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            start_delay: Duration::from_secs(60),
            tick: Duration::from_secs(60),
            pace: Duration::from_secs(2),
            ring_window: Duration::from_secs(10),
            report_every: Duration::from_secs(3600),
        }
    }
}

/// Why the poll ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// `tg serve` is shutting down.
    Shutdown,
    /// Something of a chat was read between its first read and the one after its messages were
    /// reported. Nothing more is polled until `tg serve` restarts.
    Dropped { before: ReadState, after: ReadState },
    /// A chat's read state could not be read after its messages were reported. Nothing more is
    /// polled until `tg serve` restarts, since nothing would show a read.
    Unverified,
}

/// Counts since the last summary. Numbers only: no chat, message or name is ever logged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub rounds: u64,
    pub polled: u64,
    pub nothing_to_report: u64,
    pub failed: u64,
    pub reported: u64,
    pub with_reactions: u64,
    /// Polls of a chat with at least one message TDLib held reactions for: each such poll costs
    /// one `getMessagesReactions` per 100 of them.
    pub polls_with_reactions: u64,
    /// `message_reactions` frames from a chat within [`Timing::ring_window`] of its poll.
    pub rings: u64,
}

impl Stats {
    fn add(&mut self, other: &Stats) {
        self.rounds += other.rounds;
        self.polled += other.polled;
        self.nothing_to_report += other.nothing_to_report;
        self.failed += other.failed;
        self.reported += other.reported;
        self.with_reactions += other.with_reactions;
        self.polls_with_reactions += other.polls_with_reactions;
        self.rings += other.rings;
    }
}

/// The poll's state across rounds.
pub struct Poller<S: PollSource + 'static> {
    client: Arc<Mutex<S>>,
    updates: Option<broadcast::Receiver<Update>>,
    shutdown: watch::Receiver<bool>,
    timing: Timing,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    last_polled: HashMap<i64, Instant>,
    polled_at: HashMap<i64, Instant>,
    /// Chats whose `closeChat` failed: closed again before every round and when the poll ends.
    left_open: HashSet<i64>,
    errors_said: HashSet<String>,
    stats: Stats,
    rounds_run: u64,
    last_report: Instant,
}

impl<S: PollSource + 'static> Poller<S> {
    /// `clock` is the time in Unix seconds, against which a message's date is read.
    pub fn new(
        client: Arc<Mutex<S>>,
        updates: broadcast::Receiver<Update>,
        shutdown: watch::Receiver<bool>,
        timing: Timing,
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            client,
            updates: Some(updates),
            shutdown,
            timing,
            clock,
            last_polled: HashMap::new(),
            polled_at: HashMap::new(),
            left_open: HashSet::new(),
            errors_said: HashSet::new(),
            stats: Stats::default(),
            rounds_run: 0,
            last_report: Instant::now(),
        }
    }

    /// Poll until serve shuts down or a poll shows a read. One round at a time, one chat at a
    /// time: a poll never overlaps another.
    pub async fn run(mut self) -> Stop {
        eprintln!(
            "tg serve: reaction poll on: groups with a message in the last 7 days, every 5, 20 \
             or 60 min by their last message's age ({ENV}=off turns it off)"
        );
        let stop = self.rounds().await;
        self.close_left_open().await;
        match stop {
            Stop::Shutdown => {}
            Stop::Dropped { before, after } => eprintln!(
                "tg serve: reaction poll STOPPED: something of a chat was read while it was \
                 polled ({before} before, {after} after; the inbox's read mark {}); nothing more \
                 is polled until tg serve restarts",
                if after.last_read_inbox_message_id > before.last_read_inbox_message_id {
                    "moved"
                } else {
                    "stayed"
                }
            ),
            Stop::Unverified => eprintln!(
                "tg serve: reaction poll STOPPED: a chat's read state could not be read after its \
                 messages were reported; nothing more is polled until tg serve restarts"
            ),
        }
        stop
    }

    async fn rounds(&mut self) -> Stop {
        if !self.wait(self.timing.start_delay).await {
            return Stop::Shutdown;
        }
        loop {
            let started = Instant::now();
            if let Some(stop) = self.round().await {
                return stop;
            }
            if !self.wait_until(started + self.timing.tick).await {
                return Stop::Shutdown;
            }
        }
    }

    /// One round: every group whose interval has passed since its last poll, the most recently
    /// active first.
    async fn round(&mut self) -> Option<Stop> {
        self.close_left_open().await;
        let now_unix = (self.clock)();
        let now = Instant::now();
        let mut round = Stats {
            rounds: 1,
            ..Stats::default()
        };
        let ids = {
            let client = self.client.lock().await;
            client.poll_chat_ids().await
        };
        let ids = match ids {
            Ok(ids) => ids,
            Err(e) => {
                round.failed += 1;
                self.say_error("listing chats", &e);
                self.stats.add(&round);
                return None;
            }
        };

        let mut seen = HashSet::new();
        let mut census = Totals::default();
        let mut groups = 0usize;
        let mut due: Vec<(i64, i64)> = Vec::new();
        for chat_id in ids {
            if *self.shutdown.borrow() {
                return Some(Stop::Shutdown);
            }
            if !seen.insert(chat_id) {
                continue;
            }
            let state = {
                let client = self.client.lock().await;
                client.poll_chat_state(chat_id).await
            };
            let state = match state {
                Ok(state) => state,
                Err(e) => {
                    round.failed += 1;
                    self.say_error("reading a chat", &e);
                    continue;
                }
            };
            let Some(every) = state.interval(now_unix) else {
                continue;
            };
            groups += 1;
            census.add(&state.read);
            // Half a tick of slack: a round starts a hair after the one an interval earlier
            // would have, and must still find the chat due.
            let is_due = self
                .last_polled
                .get(&chat_id)
                .is_none_or(|at| now.duration_since(*at) + self.timing.tick / 2 >= every);
            if is_due {
                let age = now_unix - state.last_message_date.unwrap_or(now_unix);
                due.push((age, chat_id));
            }
        }
        due.sort();

        let mut before = Totals::default();
        let mut after = Totals::default();
        for (_, chat_id) in due {
            if *self.shutdown.borrow() {
                return Some(Stop::Shutdown);
            }
            let outcome = {
                let client = self.client.lock().await;
                poll_chat(&*client, chat_id, (self.clock)()).await
            };
            // An interval runs from the round a chat was polled in, so it is due again in the
            // round that interval later, however far into its round it came.
            self.last_polled.insert(chat_id, now);
            match outcome {
                Outcome::NothingToReport => round.nothing_to_report += 1,
                Outcome::Failed(e) => {
                    round.failed += 1;
                    self.say_error("reading a chat to poll", &e);
                }
                Outcome::Polled {
                    poll,
                    error,
                    closed,
                } => {
                    if !closed {
                        self.left_open.insert(chat_id);
                    }
                    if poll.read_something() {
                        return Some(Stop::Dropped {
                            before: poll.before,
                            after: poll.after,
                        });
                    }
                    match error {
                        Some(e) => {
                            round.failed += 1;
                            self.say_error("polling a chat", &e);
                        }
                        None => {
                            round.polled += 1;
                            round.reported += poll.reported as u64;
                            round.with_reactions += poll.with_reactions as u64;
                            round.polls_with_reactions += u64::from(poll.with_reactions > 0);
                            before.add(&poll.before);
                            after.add(&poll.after);
                            self.polled_at.insert(chat_id, Instant::now());
                        }
                    }
                }
                Outcome::Unverified(e) => {
                    self.left_open.insert(chat_id);
                    self.say_error("reading a chat after polling it", &e);
                    return Some(Stop::Unverified);
                }
            }
            if !self.wait(self.timing.pace).await {
                return Some(Stop::Shutdown);
            }
        }

        if self.rounds_run == 0 {
            eprintln!(
                "tg serve: reaction poll, first round: {groups} groups in the window ({census} \
                 at the round's start); polled {}, {} with nothing to report, {} failed; \
                 reported {} messages, {} with reactions; in the polled chats {before} before, \
                 {after} after",
                round.polled,
                round.nothing_to_report,
                round.failed,
                round.reported,
                round.with_reactions,
            );
        }
        self.rounds_run += 1;
        self.stats.add(&round);
        if now.duration_since(self.last_report) >= self.timing.report_every {
            let s = std::mem::take(&mut self.stats);
            eprintln!(
                "tg serve: reaction poll, since the last summary: {} rounds, {} polls ({} with \
                 reactions to re-read), {} with nothing to report, {} failed; {} messages \
                 reported, {} with reactions; {} message_reactions frames within {} s of a poll; \
                 now {groups} groups in the window ({census})",
                s.rounds,
                s.polled,
                s.polls_with_reactions,
                s.nothing_to_report,
                s.failed,
                s.reported,
                s.with_reactions,
                s.rings,
                self.timing.ring_window.as_secs(),
            );
            self.last_report = now;
        }
        None
    }

    /// Close again every chat whose `closeChat` failed. One TDLib leaves open receives all of its
    /// updates; it is no read, but it is not what the poll is for.
    async fn close_left_open(&mut self) {
        let chats: Vec<i64> = self.left_open.iter().copied().collect();
        for chat_id in chats {
            let closed = {
                let client = self.client.lock().await;
                client.poll_close_chat(chat_id).await
            };
            match closed {
                Ok(()) => {
                    self.left_open.remove(&chat_id);
                }
                Err(e) => self.say_error("closing a chat again", &e),
            }
        }
    }

    /// Say an error the first time its text is seen. TDLib's errors for these calls are fixed
    /// texts and numbers ("Chat not found", "Too Many Requests: retry after 5"), never a message
    /// or a name.
    fn say_error(&mut self, doing: &str, e: &TgError) {
        let text = format!("{doing}: {e}");
        if self.errors_said.len() < 64 && self.errors_said.insert(text.clone()) {
            eprintln!("tg serve: reaction poll: {text} (said once; counted in the summary)");
        }
    }

    fn count_ring(&mut self, chat_id: i64) {
        let window = self.timing.ring_window;
        if let Some(at) = self.polled_at.get(&chat_id) {
            if at.elapsed() <= window {
                self.stats.rings += 1;
            } else {
                self.polled_at.remove(&chat_id);
            }
        }
    }

    async fn wait(&mut self, duration: Duration) -> bool {
        self.wait_until(Instant::now() + duration).await
    }

    /// Wait until `deadline`, counting the frames polled chats ring meanwhile. `false` when serve
    /// shuts down first.
    async fn wait_until(&mut self, deadline: Instant) -> bool {
        loop {
            if *self.shutdown.borrow() {
                return false;
            }
            let update = async {
                match self.updates.as_mut() {
                    Some(updates) => updates.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return true,
                changed = self.shutdown.changed() => {
                    if changed.is_err() {
                        return false;
                    }
                }
                update = update => match update {
                    Ok(Update::MessageInteractionInfo(u)) => self.count_ring(u.chat_id),
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => self.updates = None,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    const NOW: i64 = 1_800_000_000;

    fn message(id: i64, age: i64) -> RecentMessage {
        RecentMessage {
            id,
            date: NOW - age,
            has_reactions: false,
            is_live_location: false,
        }
    }

    #[test]
    fn the_variable_is_on_off_or_refused() {
        assert_eq!(setting(None).unwrap(), Setting::On);
        assert_eq!(setting(Some(OsStr::new("on"))).unwrap(), Setting::On);
        assert_eq!(setting(Some(OsStr::new("off"))).unwrap(), Setting::Off);
        for value in ["", "0", "false", "OFF", "yes"] {
            assert!(setting(Some(OsStr::new(value))).is_err(), "{value:?}");
        }
    }

    #[test]
    fn a_group_is_polled_by_the_age_of_its_last_message() {
        assert_eq!(interval_for(0), Some(Duration::from_secs(300)));
        assert_eq!(interval_for(3600), Some(Duration::from_secs(300)));
        assert_eq!(interval_for(3601), Some(Duration::from_secs(1200)));
        assert_eq!(interval_for(86_400), Some(Duration::from_secs(1200)));
        assert_eq!(interval_for(86_401), Some(Duration::from_secs(3600)));
        assert_eq!(interval_for(WINDOW_SECS), Some(Duration::from_secs(3600)));
        assert_eq!(interval_for(WINDOW_SECS + 1), None);
    }

    #[test]
    fn only_a_group_that_takes_reactions_and_has_a_recent_message_is_polled() {
        let group = ChatState {
            is_group: true,
            reactions_allowed: true,
            last_message_date: Some(NOW - 60),
            read: ReadState::default(),
        };
        assert_eq!(group.interval(NOW), Some(Duration::from_secs(300)));
        let not_group = ChatState {
            is_group: false,
            ..group
        };
        assert_eq!(not_group.interval(NOW), None);
        let no_reactions = ChatState {
            reactions_allowed: false,
            ..group
        };
        assert_eq!(no_reactions.interval(NOW), None);
        let empty = ChatState {
            last_message_date: None,
            ..group
        };
        assert_eq!(empty.interval(NOW), None);
        let quiet = ChatState {
            last_message_date: Some(NOW - WINDOW_SECS - 1),
            ..group
        };
        assert_eq!(quiet.interval(NOW), None);
    }

    #[test]
    fn a_live_location_and_a_message_before_the_window_are_never_reported() {
        let since = NOW - WINDOW_SECS;
        let mut live = message(3, 10);
        live.is_live_location = true;
        let messages = vec![
            message(4, 5),
            live,
            message(2, 20),
            message(1, WINDOW_SECS + 1),
        ];
        assert_eq!(messages_to_report(&messages, since), vec![4, 2]);
    }

    #[test]
    fn at_most_a_hundred_messages_are_reported_the_newest() {
        let messages: Vec<_> = (0..150).map(|i| message(1000 - i, i)).collect();
        let ids = messages_to_report(&messages, NOW - WINDOW_SECS);
        assert_eq!(ids.len(), MAX_MESSAGES);
        assert_eq!(ids[0], 1000);
        assert_eq!(*ids.last().unwrap(), 901);
    }

    #[test]
    fn any_read_shows_and_new_messages_cannot_hide_one() {
        let before = ReadState {
            unread_messages: 5,
            unread_mentions: 1,
            unread_reactions: 2,
            marked_unread: true,
            last_read_inbox_message_id: 100,
        };
        assert!(!before.read_since(&before));
        let more = ReadState {
            unread_messages: 7,
            unread_mentions: 2,
            ..before
        };
        assert!(!more.read_since(&before), "messages arriving read nothing");
        let marked = ReadState {
            marked_unread: false,
            ..before
        };
        let unmarked_before = ReadState {
            marked_unread: false,
            ..before
        };
        assert!(
            !before.read_since(&unmarked_before),
            "marking unread is no read"
        );
        for after in [
            ReadState {
                unread_messages: 4,
                ..before
            },
            ReadState {
                unread_mentions: 0,
                ..before
            },
            ReadState {
                unread_reactions: 1,
                ..before
            },
            marked,
            // Read up to message 101 while two new ones arrived: the count rose, the mark moved.
            ReadState {
                unread_messages: 6,
                last_read_inbox_message_id: 101,
                ..before
            },
        ] {
            assert!(after.read_since(&before), "{after:?}");
        }
    }

    /// A local history answering pages as TDLib does: newest first, strictly older than the
    /// anchor, at most `limit`.
    struct Pages {
        messages: Vec<RecentMessage>,
        pages_asked: std::sync::Mutex<usize>,
    }

    #[async_trait]
    impl LocalHistory for Pages {
        async fn local_page(
            &self,
            _chat_id: i64,
            from_message_id: i64,
            limit: usize,
        ) -> Result<Vec<RecentMessage>> {
            *self.pages_asked.lock().unwrap() += 1;
            Ok(self
                .messages
                .iter()
                .filter(|m| from_message_id == 0 || m.id < from_message_id)
                .take(limit.min(30))
                .copied()
                .collect())
        }
    }

    #[tokio::test]
    async fn recent_history_is_read_page_by_page_to_the_window_or_the_limit() {
        let history = Pages {
            messages: (0..80).map(|i| message(200 - i, i * 3600)).collect(),
            pages_asked: std::sync::Mutex::new(0),
        };
        let since = NOW - 50 * 3600;
        let recent = collect_recent(&history, 1, since, MAX_MESSAGES)
            .await
            .unwrap();
        assert_eq!(recent.len(), 51, "ages 0 to 50 hours");
        assert!(recent.iter().all(|m| m.date >= since));
        assert_eq!(*history.pages_asked.lock().unwrap(), 2);

        let limited = collect_recent(&history, 1, NOW - WINDOW_SECS, 40)
            .await
            .unwrap();
        assert_eq!(limited.len(), 40);
        assert_eq!(limited[39].id, 161);
    }

    #[tokio::test]
    async fn an_empty_page_ends_the_history() {
        let history = Pages {
            messages: (0..10).map(|i| message(100 - i, i)).collect(),
            pages_asked: std::sync::Mutex::new(0),
        };
        let recent = collect_recent(&history, 1, NOW - WINDOW_SECS, MAX_MESSAGES)
            .await
            .unwrap();
        assert_eq!(recent.len(), 10);
        assert_eq!(*history.pages_asked.lock().unwrap(), 2);
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        State(i64),
        Open(i64),
        View(i64, Vec<i64>),
        Close(i64),
    }

    /// A TDLib stand-in: chats with their state and history, and every call on a chat, in order.
    #[derive(Default)]
    struct Fake {
        chats: Vec<i64>,
        states: HashMap<i64, ChatState>,
        history: HashMap<i64, Vec<RecentMessage>>,
        calls: std::sync::Mutex<Vec<Call>>,
        /// Chats whose unread messages reporting lowers by one while they are open: TDLib marking
        /// them read. Closing a chat cannot hide it from a read made before the close.
        reads_on_view: HashSet<i64>,
        open_fails: HashSet<i64>,
        view_fails: HashSet<i64>,
        close_fails: std::sync::Mutex<HashSet<i64>>,
        /// Chats whose state cannot be read once they were opened.
        state_fails_after_open: HashSet<i64>,
    }

    impl Fake {
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn count(&self, chat_id: i64, what: fn(&Call) -> Option<i64>) -> usize {
            self.calls()
                .iter()
                .filter(|c| what(c) == Some(chat_id))
                .count()
        }
    }

    fn opened_id(c: &Call) -> Option<i64> {
        match c {
            Call::Open(id) => Some(*id),
            _ => None,
        }
    }

    fn viewed_id(c: &Call) -> Option<i64> {
        match c {
            Call::View(id, _) => Some(*id),
            _ => None,
        }
    }

    fn closed_id(c: &Call) -> Option<i64> {
        match c {
            Call::Close(id) => Some(*id),
            _ => None,
        }
    }

    #[async_trait]
    impl PollSource for Fake {
        async fn poll_chat_ids(&self) -> Result<Vec<i64>> {
            Ok(self.chats.clone())
        }

        async fn poll_chat_state(&self, chat_id: i64) -> Result<ChatState> {
            let opened = self.count(chat_id, opened_id);
            let viewed = self.count(chat_id, viewed_id) as i64;
            self.calls.lock().unwrap().push(Call::State(chat_id));
            if opened > 0 && self.state_fails_after_open.contains(&chat_id) {
                return Err(TgError::TdLib("Chat not found".to_string()));
            }
            let mut state = *self
                .states
                .get(&chat_id)
                .ok_or_else(|| TgError::TdLib("Chat not found".to_string()))?;
            if self.reads_on_view.contains(&chat_id) {
                state.read.unread_messages -= viewed;
            }
            Ok(state)
        }

        async fn local_recent_messages(
            &self,
            chat_id: i64,
            since: i64,
            limit: usize,
        ) -> Result<Vec<RecentMessage>> {
            Ok(self
                .history
                .get(&chat_id)
                .map(|h| {
                    h.iter()
                        .filter(|m| m.date >= since)
                        .take(limit)
                        .copied()
                        .collect()
                })
                .unwrap_or_default())
        }

        async fn poll_open_chat(&self, chat_id: i64) -> Result<()> {
            self.calls.lock().unwrap().push(Call::Open(chat_id));
            if self.open_fails.contains(&chat_id) {
                return Err(TgError::TdLib("Chat not found".to_string()));
            }
            Ok(())
        }

        async fn report_viewed(&self, chat_id: i64, message_ids: &[i64]) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::View(chat_id, message_ids.to_vec()));
            if self.view_fails.contains(&chat_id) {
                return Err(TgError::TdLib("Chat is not accessible".to_string()));
            }
            Ok(())
        }

        async fn poll_close_chat(&self, chat_id: i64) -> Result<()> {
            self.calls.lock().unwrap().push(Call::Close(chat_id));
            if self.close_fails.lock().unwrap().remove(&chat_id) {
                return Err(TgError::TdLib("Chat not found".to_string()));
            }
            Ok(())
        }
    }

    fn group(last_age: i64, unread: i64) -> ChatState {
        ChatState {
            is_group: true,
            reactions_allowed: true,
            last_message_date: Some(NOW - last_age),
            read: ReadState {
                unread_messages: unread,
                unread_mentions: 1,
                unread_reactions: 1,
                marked_unread: false,
                last_read_inbox_message_id: 0,
            },
        }
    }

    fn one_chat(fake: &mut Fake) {
        fake.states.insert(-5, group(60, 3));
        fake.history.insert(-5, vec![message(1, 60)]);
    }

    #[tokio::test]
    async fn a_poll_reads_the_state_opens_reports_reads_again_and_closes() {
        let mut fake = Fake::default();
        fake.states.insert(-5, group(60, 3));
        let mut reacted = message(11, 60);
        reacted.has_reactions = true;
        fake.history
            .insert(-5, vec![reacted, message(10, 120), message(9, 180)]);
        let Outcome::Polled {
            poll,
            error: None,
            closed: true,
        } = poll_chat(&fake, -5, NOW).await
        else {
            panic!("not polled cleanly");
        };
        assert_eq!(
            fake.calls(),
            vec![
                Call::State(-5),
                Call::Open(-5),
                Call::View(-5, vec![11, 10, 9]),
                Call::State(-5),
                Call::Close(-5),
            ]
        );
        assert_eq!(poll.reported, 3);
        assert_eq!(poll.with_reactions, 1);
        assert!(!poll.read_something());
    }

    #[tokio::test]
    async fn a_chat_with_nothing_to_report_is_not_opened() {
        let mut fake = Fake::default();
        fake.states.insert(-5, group(60, 3));
        let mut live = message(3, 60);
        live.is_live_location = true;
        fake.history.insert(-5, vec![live]);
        assert!(matches!(
            poll_chat(&fake, -5, NOW).await,
            Outcome::NothingToReport
        ));
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn a_chat_is_closed_when_opening_or_reporting_fails() {
        for fails_open in [true, false] {
            let mut fake = Fake::default();
            one_chat(&mut fake);
            if fails_open {
                fake.open_fails.insert(-5);
            } else {
                fake.view_fails.insert(-5);
            }
            let outcome = poll_chat(&fake, -5, NOW).await;
            assert!(matches!(
                outcome,
                Outcome::Polled {
                    error: Some(_),
                    closed: true,
                    ..
                }
            ));
            assert_eq!(fake.count(-5, closed_id), 1);
            assert_eq!(fake.count(-5, viewed_id), usize::from(!fails_open));
        }
    }

    #[tokio::test]
    async fn a_read_is_seen_before_the_chat_is_closed_even_when_reporting_failed() {
        let mut fake = Fake::default();
        one_chat(&mut fake);
        fake.reads_on_view.insert(-5);
        fake.view_fails.insert(-5);
        let Outcome::Polled { poll, error, .. } = poll_chat(&fake, -5, NOW).await else {
            panic!("not polled");
        };
        assert!(error.is_some());
        assert_eq!(poll.before.unread_messages, 3);
        assert_eq!(poll.after.unread_messages, 2);
        assert!(poll.read_something());
        let calls = fake.calls();
        let after_state = calls.iter().rposition(|c| *c == Call::State(-5)).unwrap();
        let close = calls.iter().position(|c| *c == Call::Close(-5)).unwrap();
        assert!(after_state < close, "{calls:?}");
    }

    #[tokio::test]
    async fn a_state_that_cannot_be_read_afterwards_is_unverified() {
        let mut fake = Fake::default();
        one_chat(&mut fake);
        fake.state_fails_after_open.insert(-5);
        assert!(matches!(
            poll_chat(&fake, -5, NOW).await,
            Outcome::Unverified(_)
        ));
        assert_eq!(fake.count(-5, closed_id), 1);
    }

    fn timing() -> Timing {
        Timing {
            start_delay: Duration::from_secs(60),
            tick: Duration::from_secs(60),
            pace: Duration::from_secs(2),
            ring_window: Duration::from_secs(10),
            report_every: Duration::from_secs(3600),
        }
    }

    struct Running {
        fake: Arc<Mutex<Fake>>,
        _updates: broadcast::Sender<Update>,
        stop: watch::Sender<bool>,
        task: tokio::task::JoinHandle<Stop>,
    }

    /// Start a poller on `fake` and let it begin waiting, so its start delay runs from now.
    async fn start(fake: Fake, clock: Arc<AtomicI64>) -> Running {
        let fake = Arc::new(Mutex::new(fake));
        let (updates, receiver) = broadcast::channel(64);
        let (stop, shutdown) = watch::channel(false);
        let poller = Poller::new(
            fake.clone(),
            receiver,
            shutdown,
            timing(),
            Box::new(move || clock.load(Ordering::SeqCst)),
        );
        let task = tokio::spawn(poller.run());
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        Running {
            fake,
            _updates: updates,
            stop,
            task,
        }
    }

    async fn opened(fake: &Arc<Mutex<Fake>>) -> Vec<i64> {
        fake.lock()
            .await
            .calls()
            .iter()
            .filter_map(opened_id)
            .collect()
    }

    /// Advance the paused clock by `secs`, a second at a time, so every sleep due meanwhile runs.
    async fn advance(clock: &AtomicI64, secs: u64) {
        for _ in 0..secs {
            clock.fetch_add(1, Ordering::SeqCst);
            tokio::time::advance(Duration::from_secs(1)).await;
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        }
    }

    fn fake_with(chats: &[(i64, i64)]) -> Fake {
        let mut fake = Fake::default();
        for &(chat_id, last_age) in chats {
            fake.chats.push(chat_id);
            fake.states.insert(chat_id, group(last_age, 0));
            fake.history.insert(chat_id, vec![message(1, last_age)]);
        }
        fake
    }

    #[tokio::test(start_paused = true)]
    async fn each_group_is_polled_at_its_tier_the_most_recent_first() {
        let clock = Arc::new(AtomicI64::new(NOW));
        // A chat two hours quiet, one a minute, one three days, one older and a one-to-one chat.
        let mut fake = fake_with(&[(-1, 2 * 3600), (-2, 60), (-3, 3 * 86_400)]);
        fake.chats.push(-4);
        fake.states.insert(-4, group(WINDOW_SECS + 10, 0));
        fake.chats.push(7);
        fake.states.insert(
            7,
            ChatState {
                is_group: false,
                ..group(60, 0)
            },
        );
        let running = start(fake, clock.clone()).await;

        advance(&clock, 59).await;
        assert!(
            opened(&running.fake).await.is_empty(),
            "nothing before the start delay"
        );
        advance(&clock, 10).await;
        assert_eq!(opened(&running.fake).await, vec![-2, -1, -3]);

        // At 5 minutes only the recent chat is due again, at 20 the two-hour one too, and the
        // three-day one at 60.
        advance(&clock, 5 * 60).await;
        assert_eq!(opened(&running.fake).await, vec![-2, -1, -3, -2]);
        advance(&clock, 15 * 60).await;
        let after_20 = opened(&running.fake).await;
        assert_eq!(after_20.iter().filter(|&&id| id == -1).count(), 2);
        assert_eq!(after_20.iter().filter(|&&id| id == -3).count(), 1);
        assert_eq!(after_20.iter().filter(|&&id| id == -2).count(), 5);
        advance(&clock, 41 * 60).await;
        let after_61 = opened(&running.fake).await;
        assert_eq!(after_61.iter().filter(|&&id| id == -3).count(), 2);
        assert!(!after_61.contains(&-4) && !after_61.contains(&7));

        running.stop.send_replace(true);
        assert_eq!(running.task.await.unwrap(), Stop::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn chats_of_one_round_are_polled_one_at_a_time_paced() {
        let clock = Arc::new(AtomicI64::new(NOW));
        let running = start(fake_with(&[(-1, 60), (-2, 120), (-3, 180)]), clock.clone()).await;
        advance(&clock, 60).await;
        assert_eq!(opened(&running.fake).await, vec![-1]);
        advance(&clock, 2).await;
        assert_eq!(opened(&running.fake).await, vec![-1, -2]);
        advance(&clock, 2).await;
        assert_eq!(opened(&running.fake).await, vec![-1, -2, -3]);
        // Each chat's poll is whole before the next begins.
        let calls = running.fake.lock().await.calls();
        let polls: Vec<&Call> = calls
            .iter()
            .filter(|c| !matches!(c, Call::State(_)))
            .collect();
        for chunk in polls.chunks(3) {
            assert!(matches!(
                chunk,
                [Call::Open(a), Call::View(b, _), Call::Close(c)] if a == b && b == c
            ));
        }
        running.stop.send_replace(true);
        assert_eq!(running.task.await.unwrap(), Stop::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_stops_the_poll() {
        let clock = Arc::new(AtomicI64::new(NOW));
        let mut fake = fake_with(&[(-1, 60), (-2, 120)]);
        fake.states.get_mut(&-1).unwrap().read.unread_messages = 4;
        fake.reads_on_view.insert(-1);
        let running = start(fake, clock.clone()).await;
        advance(&clock, 61).await;
        let stop = running.task.await.unwrap();
        assert!(matches!(stop, Stop::Dropped { before, after }
            if before.unread_messages == 4 && after.unread_messages == 3));
        assert_eq!(opened(&running.fake).await, vec![-1], "nothing after it");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unverified_poll_stops_the_poll() {
        let clock = Arc::new(AtomicI64::new(NOW));
        let mut fake = fake_with(&[(-1, 60), (-2, 120)]);
        fake.state_fails_after_open.insert(-1);
        let running = start(fake, clock.clone()).await;
        advance(&clock, 61).await;
        assert_eq!(running.task.await.unwrap(), Stop::Unverified);
        assert_eq!(opened(&running.fake).await, vec![-1]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_chat_does_not_stop_the_round() {
        let clock = Arc::new(AtomicI64::new(NOW));
        let mut fake = fake_with(&[(-1, 60), (-2, 120)]);
        fake.view_fails.insert(-1);
        let running = start(fake, clock.clone()).await;
        advance(&clock, 64).await;
        assert_eq!(opened(&running.fake).await, vec![-1, -2]);
        running.stop.send_replace(true);
        assert_eq!(running.task.await.unwrap(), Stop::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn a_chat_that_did_not_close_is_closed_again_before_the_next_round() {
        let clock = Arc::new(AtomicI64::new(NOW));
        let fake = fake_with(&[(-1, 60)]);
        fake.close_fails.lock().unwrap().insert(-1);
        let running = start(fake, clock.clone()).await;
        advance(&clock, 62).await;
        assert_eq!(running.fake.lock().await.count(-1, closed_id), 1);
        advance(&clock, 60).await;
        assert_eq!(running.fake.lock().await.count(-1, closed_id), 2);
        advance(&clock, 60).await;
        assert_eq!(
            running.fake.lock().await.count(-1, closed_id),
            2,
            "once is enough"
        );
        running.stop.send_replace(true);
        assert_eq!(running.task.await.unwrap(), Stop::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_ends_the_poll_while_it_waits() {
        let clock = Arc::new(AtomicI64::new(NOW));
        let running = start(fake_with(&[(-1, 60)]), clock.clone()).await;
        advance(&clock, 5).await;
        running.stop.send_replace(true);
        assert_eq!(running.task.await.unwrap(), Stop::Shutdown);
        assert!(opened(&running.fake).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn frames_from_a_polled_chat_are_counted_within_the_window() {
        let clock = Arc::new(AtomicI64::new(NOW));
        let fake = Arc::new(Mutex::new(fake_with(&[(-1, 60)])));
        let (updates, receiver) = broadcast::channel(64);
        let (_stop, shutdown) = watch::channel(false);
        let mut poller = Poller::new(
            fake.clone(),
            receiver,
            shutdown,
            timing(),
            Box::new({
                let clock = clock.clone();
                move || clock.load(Ordering::SeqCst)
            }),
        );
        assert!(poller.round().await.is_none());
        let ring = |chat_id: i64| {
            Update::MessageInteractionInfo(tdlib_rs::types::UpdateMessageInteractionInfo {
                chat_id,
                message_id: 1,
                interaction_info: None,
            })
        };
        updates.send(ring(-1)).unwrap();
        updates.send(ring(-9)).unwrap();
        assert!(poller.wait(Duration::from_secs(1)).await);
        assert_eq!(poller.stats.rings, 1, "only the polled chat's frame");
        tokio::time::advance(Duration::from_secs(20)).await;
        updates.send(ring(-1)).unwrap();
        assert!(poller.wait(Duration::from_secs(1)).await);
        assert_eq!(poller.stats.rings, 1, "nothing after the window");
    }
}
