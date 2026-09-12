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
}

impl TgError {
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
