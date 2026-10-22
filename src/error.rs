use thiserror::Error;

#[derive(Error, Debug)]
pub enum TgError {
    #[error("Authentication required. Run 'tg auth' first.")]
    NotAuthenticated,

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Chat not found: {0}")]
    ChatNotFound(String),

    #[error("Chat {0} is inaccessible (group may be deleted or restricted)")]
    ChatInaccessible(i64),

    #[error("Contact not found: {0}")]
    ContactNotFound(String),

    #[error("Invalid phone number format")]
    InvalidPhoneNumber,

    #[error("Verification code required")]
    CodeRequired,

    #[error("2FA password required")]
    PasswordRequired,

    #[error("TDLib error: {0}")]
    TdLib(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Environment variable {0} not set")]
    EnvVarMissing(String),

    #[error("{0}")]
    Other(String),

    /// A media send that could not deliver every element, carrying the
    /// per-element record of what it DID deliver.
    ///
    /// The one error here with structured data on it, because it is the one
    /// whose failure is not the whole truth: an album is N independent Telegram
    /// messages, so a failure can coexist with elements already sitting in the
    /// recipient's chat. `serve::dispatch` answers it as
    /// `{"ok": false, "error": ..., "result": <partial>}` — the plain
    /// `ok:false` this replaces is what let a caller conclude "nothing
    /// arrived" and resend the album on top of the delivered files.
    #[error("{message}")]
    PartialSend {
        message: String,
        partial: Box<crate::output::PartialSendResult>,
    },

    /// A request refused for a reason its caller acts on, so the reason travels
    /// as a word as well as a sentence: `serve::dispatch` answers it as
    /// `{"ok": false, "code": ..., "error": ...}`. Nothing was changed.
    #[error("{message}")]
    Refused { refusal: Refusal, message: String },
}

/// Why a request was refused, as the `code` of its failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// TDLib does not know the chat (`getChat`: "Chat not found").
    NotFound,
    /// The chat holds a message later than the caller had seen.
    NewerMessages,
    /// The chat is no one-to-one chat.
    NotPrivate,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::NewerMessages => "newer_messages",
            Self::NotPrivate => "not_private",
        }
    }
}

impl TgError {
    /// The word a caller acts on, for a refusal; `None` for every other error.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Refused { refusal, .. } => Some(refusal.code()),
            _ => None,
        }
    }

    /// The structured payload a transport must report ALONGSIDE this error, if
    /// it has one. `None` for every error that left nothing behind.
    pub fn partial_send(&self) -> Option<&crate::output::PartialSendResult> {
        match self {
            Self::PartialSend { partial, .. } => Some(partial),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, TgError>;
