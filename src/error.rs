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
}

pub type Result<T> = std::result::Result<T, TgError>;
