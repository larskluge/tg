//! Reaction changes Telegram has not answered yet: one change per message at a
//! time (since 0.15.1, tavola-fkp.6).
//!
//! TDLib puts a reaction change on its own copy of the message at once and only
//! then asks Telegram, and it answers a second request for a change that copy
//! already shows at once, without asking Telegram again (AGENTS.md,
//! *Reactions*). So a request that stops waiting leaves its change here, still
//! waiting on Telegram's answer, and the next request for the same message waits
//! on that answer instead of being answered from TDLib's copy.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::watch;
use tokio::time::{Duration, timeout};

use crate::error::{Result, TgError};
use crate::reactions::emoji_key;

/// Maximum seconds a request waits for Telegram's answer to a reaction change:
/// its own, or one an earlier request asked for. Running out of time says
/// nothing about the outcome.
pub const REACTION_TIMEOUT_SECS: u64 = 10;

/// A change to the account's reaction on one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReactionChange {
    /// React with this emoji.
    Add(String),
    /// Take back this emoji; empty for every reaction of the account.
    Remove(String),
}

impl ReactionChange {
    /// Whether this asks again for what `earlier` asked: the same emoji in
    /// either spelling, and every taking back of every reaction is one.
    pub fn repeats(&self, earlier: &ReactionChange) -> bool {
        match (self, earlier) {
            (Self::Add(a), Self::Add(b)) => emoji_key(a) == emoji_key(b),
            (Self::Remove(a), Self::Remove(b)) => {
                a.is_empty() || b.is_empty() || emoji_key(a) == emoji_key(b)
            }
            _ => false,
        }
    }
}

/// Telegram's answer to a change: `Err` carries its reason for refusing it.
pub type Answer = std::result::Result<(), String>;

/// A change an earlier request asked for, with Telegram's answer to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub change: ReactionChange,
    pub answer: Answer,
}

/// What waiting on a change came to.
enum Wait {
    Answered(Answer),
    /// Telegram did not answer within [`REACTION_TIMEOUT_SECS`].
    Silent,
    /// The call ended without an answer: its task panicked or the runtime is
    /// shutting down.
    Abandoned,
}

struct Pending {
    id: u64,
    change: ReactionChange,
    answer: watch::Receiver<Option<Answer>>,
}

/// The reaction changes Telegram has not answered, by `(chat_id, message_id)`.
#[derive(Default)]
pub struct ReactionChanges {
    pending: Mutex<HashMap<(i64, i64), Pending>>,
    next_id: AtomicU64,
}

impl ReactionChanges {
    /// Ask Telegram for `change` through `call` and wait at most
    /// [`REACTION_TIMEOUT_SECS`] for its answer. The call runs on after that, and
    /// the change stays pending here until Telegram answers.
    pub async fn ask<F>(
        self: &Arc<Self>,
        chat_id: i64,
        message_id: i64,
        change: ReactionChange,
        call: F,
    ) -> Result<()>
    where
        F: Future<Output = Answer> + Send + 'static,
    {
        let key = (chat_id, message_id);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer, waiter) = watch::channel(None);
        self.lock().insert(
            key,
            Pending {
                id,
                change,
                answer: waiter.clone(),
            },
        );
        let settled = Settled {
            changes: Arc::clone(self),
            key,
            id,
            answer,
        };
        tokio::spawn(async move {
            let got = call.await;
            settled.answer.send_replace(Some(got));
            // Dropping `settled` takes the change off the pending ones only
            // now: a request that comes after finds nothing pending and asks
            // TDLib, whose copy then holds Telegram's answer.
        });
        match wait(waiter).await {
            Wait::Answered(Ok(())) => Ok(()),
            Wait::Answered(Err(reason)) => Err(TgError::TdLib(reason)),
            Wait::Silent => Err(TgError::Other(format!(
                "Telegram did not answer within {REACTION_TIMEOUT_SECS}s; the change may still take effect"
            ))),
            Wait::Abandoned => Err(abandoned()),
        }
    }

    /// The change to this message an earlier request asked for and Telegram
    /// had not answered, with its answer, waited for at most
    /// [`REACTION_TIMEOUT_SECS`]. `None` when no change to it is pending; an
    /// error when Telegram still has not answered.
    pub async fn earlier(&self, chat_id: i64, message_id: i64) -> Result<Option<Answered>> {
        let pending = self
            .lock()
            .get(&(chat_id, message_id))
            .map(|p| (p.change.clone(), p.answer.clone()));
        let Some((change, waiter)) = pending else {
            return Ok(None);
        };
        match wait(waiter).await {
            Wait::Answered(answer) => Ok(Some(Answered { change, answer })),
            Wait::Silent => Err(TgError::Other(format!(
                "Telegram has still not answered an earlier change to message {message_id} in chat {chat_id}, so nothing was asked again; that change may still take effect"
            ))),
            Wait::Abandoned => Err(abandoned()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<(i64, i64), Pending>> {
        // The map holds no invariant a panic elsewhere could break.
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn abandoned() -> TgError {
    TgError::Other(
        "tg stopped waiting for Telegram's answer before it came; the change may still take effect"
            .to_string(),
    )
}

async fn wait(mut waiter: watch::Receiver<Option<Answer>>) -> Wait {
    let deadline = Duration::from_secs(REACTION_TIMEOUT_SECS);
    match timeout(deadline, waiter.wait_for(Option::is_some)).await {
        Ok(Ok(answer)) => Wait::Answered(answer.clone().expect("waited for an answer")),
        Ok(Err(_)) => Wait::Abandoned,
        Err(_) => Wait::Silent,
    }
}

/// Takes a change off the pending ones when its call ends, however it ends: a
/// call that never answers must not hold its message up for good. The entry
/// goes before the sender does, so whoever is woken by a call that died finds
/// nothing pending.
struct Settled {
    changes: Arc<ReactionChanges>,
    key: (i64, i64),
    id: u64,
    answer: watch::Sender<Option<Answer>>,
}

impl Drop for Settled {
    fn drop(&mut self) {
        let mut pending = self.changes.lock();
        if pending.get(&self.key).is_some_and(|p| p.id == self.id) {
            pending.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: i64 = -1_009_876_543_210;
    const MSG: i64 = 918 << 20;

    fn thumbs() -> ReactionChange {
        ReactionChange::Add("👍".to_string())
    }

    #[test]
    fn a_repeat_is_the_same_emoji_in_either_spelling() {
        let heart = ReactionChange::Add("\u{2764}".to_string());
        assert!(ReactionChange::Add("\u{2764}\u{FE0F}".to_string()).repeats(&heart));
        assert!(!thumbs().repeats(&heart));
        assert!(!ReactionChange::Remove("👍".to_string()).repeats(&thumbs()));
        assert!(!thumbs().repeats(&ReactionChange::Remove("👍".to_string())));
    }

    #[test]
    fn taking_back_everything_repeats_any_taking_back() {
        let all = ReactionChange::Remove(String::new());
        let one = ReactionChange::Remove("👍".to_string());
        assert!(all.repeats(&one));
        assert!(one.repeats(&all));
        assert!(!ReactionChange::Remove("🔥".to_string()).repeats(&one));
    }

    #[tokio::test]
    async fn nothing_asked_is_nothing_pending() {
        let changes = ReactionChanges::default();
        assert_eq!(changes.earlier(CHAT, MSG).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_answered_change_leaves_nothing_pending() {
        let changes = Arc::new(ReactionChanges::default());
        changes
            .ask(CHAT, MSG, thumbs(), async { Ok(()) })
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert_eq!(changes.earlier(CHAT, MSG).await.unwrap(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_change_is_pending_for_its_message_alone() {
        let changes = Arc::new(ReactionChanges::default());
        let err = changes
            .ask(CHAT, MSG, thumbs(), std::future::pending())
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Telegram did not answer within 10s; the change may still take effect"
        );

        let err = changes.earlier(CHAT, MSG).await.unwrap_err().to_string();
        assert!(
            err.contains("still not answered an earlier change"),
            "{err}"
        );
        assert!(
            err.contains(&format!("message {MSG} in chat {CHAT}")),
            "{err}"
        );
        assert_eq!(changes.earlier(CHAT, MSG + (1 << 20)).await.unwrap(), None);
        assert_eq!(changes.earlier(CHAT + 1, MSG).await.unwrap(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_answer_reaches_the_request_waiting_on_it() {
        let changes = Arc::new(ReactionChanges::default());
        let (telegram, answer) = watch::channel(None::<Answer>);
        let call = async move {
            let mut answer = answer;
            let got = answer.wait_for(Option::is_some).await.unwrap();
            got.clone().unwrap()
        };
        assert!(changes.ask(CHAT, MSG, thumbs(), call).await.is_err());

        let later = async {
            tokio::time::sleep(Duration::from_secs(3)).await;
            telegram.send_replace(Some(Err("REACTION_INVALID".to_string())));
        };
        let (earlier, ()) = tokio::join!(changes.earlier(CHAT, MSG), later);
        assert_eq!(
            earlier.unwrap(),
            Some(Answered {
                change: thumbs(),
                answer: Err("REACTION_INVALID".to_string()),
            })
        );
        tokio::task::yield_now().await;
        assert_eq!(changes.earlier(CHAT, MSG).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_call_that_ends_without_an_answer_holds_nothing_up() {
        let changes = Arc::new(ReactionChanges::default());
        let err = changes
            .ask(CHAT, MSG, thumbs(), async {
                panic!("the call's task died")
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("stopped waiting for Telegram's answer"),
            "{err}"
        );
        assert_eq!(changes.earlier(CHAT, MSG).await.unwrap(), None);
    }
}
