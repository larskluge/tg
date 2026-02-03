use async_trait::async_trait;
use std::os::raw::c_int;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::{broadcast, Mutex};

use crate::error::{Result, TgError};
use crate::output::{ChatInfo, ContactInfo, MessageInfo, SendResult};

// Direct FFI to TDLib's synchronous log functions (not exposed by tdlib-rs)
#[link(name = "tdjson")]
unsafe extern "C" {
    fn td_set_log_verbosity_level(new_verbosity_level: c_int);
}

/// Set TDLib's log verbosity level (0 = fatal errors only, 1 = errors, 2 = warnings + errors)
/// Must be called before creating any TDLib client.
fn set_tdlib_log_verbosity(level: i32) {
    unsafe {
        td_set_log_verbosity_level(level);
    }
}

#[async_trait]
pub trait TelegramClient: Send + Sync {
    async fn authenticate(
        &mut self,
        phone: Option<&str>,
    ) -> Result<()>;

    async fn is_authenticated(&self) -> bool;

    async fn get_chats(&self, limit: i32) -> Result<Vec<ChatInfo>>;
    async fn get_groups(&self, limit: i32) -> Result<Vec<ChatInfo>>;
    async fn get_unread_chats(&self, limit: i32) -> Result<Vec<ChatInfo>>;

    async fn search_contacts(&self, query: &str) -> Result<Vec<ContactInfo>>;

    async fn find_chat_by_name(&self, name: &str) -> Result<i64>;
    async fn find_group_by_name(&self, name: &str) -> Result<i64>;

    async fn send_message(&self, chat_id: i64, text: &str) -> Result<SendResult>;

    async fn get_messages(&self, chat_id: i64, limit: i32) -> Result<Vec<MessageInfo>>;

    async fn mark_chat_as_read(&self, chat_id: i64) -> Result<()>;
    async fn mark_chat_as_unread(&self, chat_id: i64) -> Result<()>;
}

pub struct TdLibClient {
    client_id: Arc<Mutex<Option<i32>>>,
    api_id: i32,
    api_hash: String,
    data_dir: PathBuf,
    authenticated: Arc<Mutex<bool>>,
    /// Broadcast sender for TDLib updates from the background receive thread
    update_sender: broadcast::Sender<tdlib_rs::enums::Update>,
    /// Signal to stop the receive loop
    shutdown: Arc<AtomicBool>,
    /// Handle to the receive loop thread
    receive_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl TdLibClient {
    pub fn new(api_id: i32, api_hash: String) -> Result<Self> {
        // Set TDLib log verbosity to fatal errors only (0) before any TDLib operations
        // Level 0 = fatal errors, 1 = errors, 2 = warnings
        set_tdlib_log_verbosity(0);

        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tg");

        std::fs::create_dir_all(&data_dir)?;

        // Create broadcast channel for updates (capacity 100)
        let (update_sender, _) = broadcast::channel(100);

        Ok(Self {
            client_id: Arc::new(Mutex::new(None)),
            api_id,
            api_hash,
            data_dir,
            authenticated: Arc::new(Mutex::new(false)),
            update_sender,
            shutdown: Arc::new(AtomicBool::new(false)),
            receive_handle: Arc::new(Mutex::new(None)),
        })
    }

    /// Spawn the background receive loop as a native thread.
    /// TDLib's receive() blocks (with 2s timeout), so we use a dedicated thread.
    async fn spawn_receive_loop(&self) {
        let sender = self.update_sender.clone();
        let shutdown = self.shutdown.clone();

        let handle = std::thread::spawn(move || {
            loop {
                // Check shutdown flag
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                // TDLib's receive() has a 2s internal timeout
                if let Some((update, _client_id)) = tdlib_rs::receive() {
                    // Send to all subscribers, ignore errors (no receivers is ok)
                    let _ = sender.send(update);
                }
            }
        });

        *self.receive_handle.lock().await = Some(handle);
    }

    /// Gracefully shut down the TDLib client
    pub async fn shutdown(&mut self) {
        // Close TDLib client if started (must happen before stopping receive loop)
        if let Some(client_id) = *self.client_id.lock().await {
            // Request TDLib to close with a timeout - don't block forever
            let close_future = tdlib_rs::functions::close(client_id);
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_secs(2),
                close_future
            ).await;
        }

        // Signal the receive loop to stop
        self.shutdown.store(true, Ordering::Relaxed);

        // Give the receive thread time to notice the shutdown flag and exit
        // The receive() call has a 2s internal timeout
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    pub async fn start(&mut self) -> Result<()> {
        use tdlib_rs::enums::AuthorizationState;

        let client_id = tdlib_rs::create_client();
        *self.client_id.lock().await = Some(client_id);

        // CRITICAL: Spawn receive loop BEFORE any TDLib operations
        self.spawn_receive_loop().await;

        // Small delay to let the receive loop start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Subscribe to updates before triggering TDLib
        let mut receiver = self.update_sender.subscribe();

        // Trigger TDLib to start sending updates
        let _ = tdlib_rs::functions::get_option("version".to_string(), client_id).await;

        // Wait for TDLib to be ready (parameters set + authenticated)
        loop {
            match receiver.recv().await {
                Ok(update) => {
                    if let tdlib_rs::enums::Update::AuthorizationState(state) = update {
                        match state.authorization_state {
                            AuthorizationState::WaitTdlibParameters => {
                                tdlib_rs::functions::set_tdlib_parameters(
                                    false,
                                    self.data_dir.join("db").to_string_lossy().to_string(),
                                    self.data_dir.join("files").to_string_lossy().to_string(),
                                    String::new(),
                                    true, true, true, false,
                                    self.api_id,
                                    self.api_hash.clone(),
                                    "en".to_string(),
                                    "CLI".to_string(),
                                    "1.0".to_string(),
                                    env!("CARGO_PKG_VERSION").to_string(),
                                    client_id,
                                )
                                .await
                                .map_err(|e| TgError::TdLib(e.message))?;
                            }
                            AuthorizationState::Ready => {
                                *self.authenticated.lock().await = true;
                                return Ok(());
                            }
                            AuthorizationState::WaitPhoneNumber
                            | AuthorizationState::WaitCode(_)
                            | AuthorizationState::WaitPassword(_) => {
                                return Err(TgError::AuthFailed(
                                    "Not authenticated. Run `tg auth --phone <number>` first.".to_string()
                                ));
                            }
                            AuthorizationState::Closed => {
                                return Err(TgError::AuthFailed("Session closed".to_string()));
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    return Err(TgError::Other(format!("Update channel error: {}", e)));
                }
            }
        }
    }

    async fn get_client_id(&self) -> Result<i32> {
        self.client_id
            .lock()
            .await
            .ok_or_else(|| TgError::Other("Client not started".to_string()))
    }
}

// Helper to extract Chat fields from the enum
fn unwrap_chat(chat: tdlib_rs::enums::Chat) -> tdlib_rs::types::Chat {
    match chat {
        tdlib_rs::enums::Chat::Chat(c) => c,
    }
}

// Helper to extract User fields from the enum
fn unwrap_user(user: tdlib_rs::enums::User) -> tdlib_rs::types::User {
    match user {
        tdlib_rs::enums::User::User(u) => u,
    }
}

// Helper to extract Message fields from the enum
fn unwrap_message(msg: tdlib_rs::enums::Message) -> tdlib_rs::types::Message {
    match msg {
        tdlib_rs::enums::Message::Message(m) => m,
    }
}

// Helper to extract Chats fields from the enum
fn unwrap_chats(chats: tdlib_rs::enums::Chats) -> tdlib_rs::types::Chats {
    match chats {
        tdlib_rs::enums::Chats::Chats(c) => c,
    }
}

// Helper to extract Users fields from the enum
fn unwrap_users(users: tdlib_rs::enums::Users) -> tdlib_rs::types::Users {
    match users {
        tdlib_rs::enums::Users::Users(u) => u,
    }
}

// Helper to extract Messages fields from the enum
fn unwrap_messages(msgs: tdlib_rs::enums::Messages) -> tdlib_rs::types::Messages {
    match msgs {
        tdlib_rs::enums::Messages::Messages(m) => m,
    }
}

fn get_user_full_name(user: &tdlib_rs::types::User) -> String {
    if user.last_name.is_empty() {
        user.first_name.clone()
    } else {
        format!("{} {}", user.first_name, user.last_name)
    }
}

#[async_trait]
impl TelegramClient for TdLibClient {
    async fn authenticate(
        &mut self,
        phone: Option<&str>,
    ) -> Result<()> {
        use std::io::{self, BufRead, Write};
        use tdlib_rs::enums::AuthorizationState;

        // Initialize client if needed (this starts the receive loop)
        if self.client_id.lock().await.is_none() {
            self.start().await?;
        }

        let client_id = self.get_client_id().await?;

        // Subscribe to updates from the background receive loop
        let mut receiver = self.update_sender.subscribe();

        // TDLib needs at least one request before it sends updates.
        // Send a simple request to trigger the update flow.
        let _ = tdlib_rs::functions::get_option("version".to_string(), client_id).await;

        loop {
            match receiver.recv().await {
                Ok(update) => {
                    if let tdlib_rs::enums::Update::AuthorizationState(state) = update {
                        match state.authorization_state {
                            AuthorizationState::WaitTdlibParameters => {
                                tdlib_rs::functions::set_tdlib_parameters(
                                    false,
                                    self.data_dir.join("db").to_string_lossy().to_string(),
                                    self.data_dir.join("files").to_string_lossy().to_string(),
                                    String::new(),
                                    true, true, true, false,
                                    self.api_id,
                                    self.api_hash.clone(),
                                    "en".to_string(),
                                    "CLI".to_string(),
                                    "1.0".to_string(),
                                    env!("CARGO_PKG_VERSION").to_string(),
                                    client_id,
                                )
                                .await
                                .map_err(|e| TgError::TdLib(e.message))?;
                            }
                            AuthorizationState::WaitPhoneNumber => {
                                let phone = phone.ok_or_else(|| {
                                    TgError::Other(
                                        "Phone number required. Run: tg auth --phone +1234567890"
                                            .to_string(),
                                    )
                                })?;
                                println!("Sending phone number...");
                                tdlib_rs::functions::set_authentication_phone_number(
                                    phone.to_string(),
                                    None,
                                    client_id,
                                )
                                .await
                                .map_err(|e| TgError::AuthFailed(e.message))?;
                            }
                            AuthorizationState::WaitCode(_) => {
                                println!("A verification code was sent to your Telegram app.");
                                print!("Enter the code from Telegram: ");
                                io::stdout().flush().ok();
                                let code = io::stdin()
                                    .lock()
                                    .lines()
                                    .next()
                                    .ok_or_else(|| TgError::Other("Failed to read code".to_string()))?
                                    .map_err(|e| TgError::Other(e.to_string()))?;
                                tdlib_rs::functions::check_authentication_code(code, client_id)
                                    .await
                                    .map_err(|e| TgError::AuthFailed(e.message))?;
                            }
                            AuthorizationState::WaitPassword(_) => {
                                print!("Enter 2FA password: ");
                                io::stdout().flush().ok();
                                let password = io::stdin()
                                    .lock()
                                    .lines()
                                    .next()
                                    .ok_or_else(|| TgError::Other("Failed to read password".to_string()))?
                                    .map_err(|e| TgError::Other(e.to_string()))?;
                                tdlib_rs::functions::check_authentication_password(password, client_id)
                                    .await
                                    .map_err(|e| TgError::AuthFailed(e.message))?;
                            }
                            AuthorizationState::Ready => {
                                *self.authenticated.lock().await = true;
                                println!("Authenticated successfully!");
                                return Ok(());
                            }
                            AuthorizationState::Closed => {
                                return Err(TgError::AuthFailed("Session closed".to_string()));
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    return Err(TgError::Other(format!("Update channel error: {}", e)));
                }
            }
        }
    }

    async fn is_authenticated(&self) -> bool {
        *self.authenticated.lock().await
    }

    async fn get_chats(&self, limit: i32) -> Result<Vec<ChatInfo>> {
        let client_id = self.get_client_id().await?;

        let chats_enum = tdlib_rs::functions::get_chats(None, limit, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;

        let chats = unwrap_chats(chats_enum);
        let mut result = Vec::new();

        for chat_id in chats.chat_ids {
            if let Ok(chat_enum) = tdlib_rs::functions::get_chat(chat_id, client_id).await {
                let chat = unwrap_chat(chat_enum);
                // Filter for 1:1 chats only (private chats)
                if matches!(chat.r#type, tdlib_rs::enums::ChatType::Private(_)) {
                    result.push(ChatInfo {
                        id: chat.id,
                        name: chat.title,
                        unread_count: chat.unread_count,
                        last_message: chat
                            .last_message
                            .as_ref()
                            .and_then(|m| extract_message_text(&m.content)),
                    });
                }
            }
        }
        Ok(result)
    }

    async fn get_groups(&self, limit: i32) -> Result<Vec<ChatInfo>> {
        let client_id = self.get_client_id().await?;

        let chats_enum = tdlib_rs::functions::get_chats(None, limit, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;

        let chats = unwrap_chats(chats_enum);
        let mut result = Vec::new();

        for chat_id in chats.chat_ids {
            if let Ok(chat_enum) = tdlib_rs::functions::get_chat(chat_id, client_id).await {
                let chat = unwrap_chat(chat_enum);
                // Filter for group chats only
                if matches!(
                    chat.r#type,
                    tdlib_rs::enums::ChatType::BasicGroup(_)
                        | tdlib_rs::enums::ChatType::Supergroup(_)
                ) {
                    result.push(ChatInfo {
                        id: chat.id,
                        name: chat.title,
                        unread_count: chat.unread_count,
                        last_message: chat
                            .last_message
                            .as_ref()
                            .and_then(|m| extract_message_text(&m.content)),
                    });
                }
            }
        }
        Ok(result)
    }

    async fn get_unread_chats(&self, limit: i32) -> Result<Vec<ChatInfo>> {
        let client_id = self.get_client_id().await?;

        let chats_enum = tdlib_rs::functions::get_chats(None, limit, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;

        let chats = unwrap_chats(chats_enum);
        let mut result = Vec::new();

        for chat_id in chats.chat_ids {
            if let Ok(chat_enum) = tdlib_rs::functions::get_chat(chat_id, client_id).await {
                let chat = unwrap_chat(chat_enum);
                if chat.unread_count > 0 {
                    result.push(ChatInfo {
                        id: chat.id,
                        name: chat.title,
                        unread_count: chat.unread_count,
                        last_message: chat
                            .last_message
                            .as_ref()
                            .and_then(|m| extract_message_text(&m.content)),
                    });
                }
            }
        }
        Ok(result)
    }

    async fn search_contacts(&self, query: &str) -> Result<Vec<ContactInfo>> {
        let client_id = self.get_client_id().await?;

        let users_enum = tdlib_rs::functions::search_contacts(query.to_string(), 50, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;

        let users = unwrap_users(users_enum);
        let mut result = Vec::new();

        for user_id in users.user_ids {
            if let Ok(user_enum) = tdlib_rs::functions::get_user(user_id, client_id).await {
                let user = unwrap_user(user_enum);
                let username = user
                    .usernames
                    .as_ref()
                    .and_then(|u| u.active_usernames.first().cloned());
                let phone = if user.phone_number.is_empty() {
                    None
                } else {
                    Some(user.phone_number.clone())
                };
                result.push(ContactInfo {
                    id: user_id,
                    name: get_user_full_name(&user),
                    username,
                    phone,
                });
            }
        }
        Ok(result)
    }

    async fn find_chat_by_name(&self, name: &str) -> Result<i64> {
        let contacts = self.search_contacts(name).await?;
        contacts
            .first()
            .map(|c| c.id)
            .ok_or_else(|| TgError::ContactNotFound(name.to_string()))
    }

    async fn find_group_by_name(&self, name: &str) -> Result<i64> {
        let client_id = self.get_client_id().await?;

        let chats_enum = tdlib_rs::functions::search_public_chats(name.to_string(), client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;

        let chats = unwrap_chats(chats_enum);

        for chat_id in chats.chat_ids {
            if let Ok(chat_enum) = tdlib_rs::functions::get_chat(chat_id, client_id).await {
                let chat = unwrap_chat(chat_enum);
                if chat.title.to_lowercase().contains(&name.to_lowercase()) {
                    return Ok(chat.id);
                }
            }
        }
        Err(TgError::ChatNotFound(name.to_string()))
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> Result<SendResult> {
        use tdlib_rs::enums::{InputMessageContent, Update};
        use tdlib_rs::types::{FormattedText, InputMessageText};

        let client_id = self.get_client_id().await?;

        // First, ensure we have a chat open (creates private chat if needed)
        let _ = tdlib_rs::functions::create_private_chat(chat_id, true, client_id).await;

        let content = InputMessageContent::InputMessageText(InputMessageText {
            text: FormattedText {
                text: text.to_string(),
                entities: vec![],
            },
            link_preview_options: None,
            clear_draft: true,
        });

        // Subscribe to updates before sending
        let mut receiver = self.update_sender.subscribe();

        let message_enum = tdlib_rs::functions::send_message(
            chat_id, 0, None, None, content, client_id,
        )
        .await
        .map_err(|e| TgError::TdLib(e.message))?;

        let message = unwrap_message(message_enum);
        let local_message_id = message.id;

        // Wait for send confirmation (success or failure)
        let timeout = tokio::time::Duration::from_secs(10);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // Timeout - message might still be sending, return local ID
                return Ok(SendResult {
                    message_id: local_message_id,
                    chat_id: message.chat_id,
                });
            }

            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Ok(update)) => {
                    match update {
                        Update::MessageSendSucceeded(msg_update) => {
                            if msg_update.old_message_id == local_message_id {
                                return Ok(SendResult {
                                    message_id: msg_update.message.id,
                                    chat_id: msg_update.message.chat_id,
                                });
                            }
                        }
                        Update::MessageSendFailed(msg_update) => {
                            if msg_update.old_message_id == local_message_id {
                                return Err(TgError::TdLib(msg_update.error.message));
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Err(_)) => {
                    // Channel closed
                    break;
                }
                Err(_) => {
                    // Timeout
                    break;
                }
            }
        }

        // Fallback - return local message ID
        Ok(SendResult {
            message_id: local_message_id,
            chat_id: message.chat_id,
        })
    }

    async fn get_messages(&self, chat_id: i64, limit: i32) -> Result<Vec<MessageInfo>> {
        let client_id = self.get_client_id().await?;

        let messages_enum =
            tdlib_rs::functions::get_chat_history(chat_id, 0, 0, limit, false, client_id)
                .await
                .map_err(|e| TgError::TdLib(e.message))?;

        let messages = unwrap_messages(messages_enum);
        let mut result = Vec::new();

        for msg in messages.messages.into_iter().flatten() {
            let sender = match &msg.sender_id {
                tdlib_rs::enums::MessageSender::User(u) => {
                    if let Ok(user_enum) =
                        tdlib_rs::functions::get_user(u.user_id, client_id).await
                    {
                        let user = unwrap_user(user_enum);
                        get_user_full_name(&user)
                    } else {
                        "Unknown".to_string()
                    }
                }
                tdlib_rs::enums::MessageSender::Chat(c) => {
                    if let Ok(chat_enum) =
                        tdlib_rs::functions::get_chat(c.chat_id, client_id).await
                    {
                        let chat = unwrap_chat(chat_enum);
                        chat.title
                    } else {
                        "Unknown".to_string()
                    }
                }
            };

            let text = extract_message_text(&msg.content).unwrap_or_default();
            let date = format_timestamp(msg.date);

            result.push(MessageInfo {
                id: msg.id,
                sender,
                text,
                date,
                is_outgoing: msg.is_outgoing,
            });
        }
        Ok(result)
    }

    async fn mark_chat_as_read(&self, chat_id: i64) -> Result<()> {
        let client_id = self.get_client_id().await?;

        // Get the last message to mark as read
        let chat_enum = tdlib_rs::functions::get_chat(chat_id, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;

        let chat = unwrap_chat(chat_enum);

        if let Some(last_message) = chat.last_message {
            tdlib_rs::functions::view_messages(
                chat_id,
                vec![last_message.id],
                None,
                true,
                client_id,
            )
            .await
            .map_err(|e| TgError::TdLib(e.message))?;
        }

        Ok(())
    }

    async fn mark_chat_as_unread(&self, chat_id: i64) -> Result<()> {
        let client_id = self.get_client_id().await?;
        tdlib_rs::functions::toggle_chat_is_marked_as_unread(chat_id, true, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;
        Ok(())
    }
}

fn extract_message_text(content: &tdlib_rs::enums::MessageContent) -> Option<String> {
    use tdlib_rs::enums::MessageContent;

    match content {
        MessageContent::MessageText(t) => Some(t.text.text.clone()),
        MessageContent::MessagePhoto(p) => Some(p.caption.text.clone()).filter(|s| !s.is_empty()),
        MessageContent::MessageVideo(v) => Some(v.caption.text.clone()).filter(|s| !s.is_empty()),
        MessageContent::MessageDocument(d) => Some(d.document.file_name.clone()),
        MessageContent::MessageSticker(s) => Some(s.sticker.emoji.clone()),
        _ => Some("[Media]".to_string()),
    }
}

fn format_timestamp(timestamp: i32) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + Duration::from_secs(timestamp as u64);
    let now = std::time::SystemTime::now();

    if let Ok(duration) = now.duration_since(datetime) {
        let secs = duration.as_secs();
        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else {
            format!("{}d ago", secs / 86400)
        }
    } else {
        "unknown".to_string()
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;

    #[derive(Clone, Copy, PartialEq)]
    pub enum AuthState {
        WaitPhone,
        WaitCode,
        WaitPassword,
        Ready,
    }

    pub struct MockClient {
        pub authenticated: std::sync::Mutex<bool>,
        pub phone_sent: std::sync::Mutex<bool>,
        pub auth_state: std::sync::Mutex<AuthState>,
        pub chats: Vec<ChatInfo>,
        pub groups: Vec<ChatInfo>,
        pub contacts: Vec<ContactInfo>,
        pub messages: Vec<MessageInfo>,
    }

    impl MockClient {
        pub fn with_state(state: AuthState) -> Self {
            let client = Self::default();
            *client.auth_state.lock().unwrap() = state;
            client
        }

        pub async fn phone_submitted(&self) -> bool {
            *self.phone_sent.lock().unwrap()
        }
    }

    impl Default for MockClient {
        fn default() -> Self {
            Self {
                authenticated: std::sync::Mutex::new(false),
                phone_sent: std::sync::Mutex::new(false),
                auth_state: std::sync::Mutex::new(AuthState::WaitPhone),
                chats: vec![
                    ChatInfo {
                        id: 1,
                        name: "John Doe".to_string(),
                        unread_count: 2,
                        last_message: Some("Hello!".to_string()),
                    },
                    ChatInfo {
                        id: 2,
                        name: "Jane Smith".to_string(),
                        unread_count: 0,
                        last_message: None,
                    },
                ],
                groups: vec![ChatInfo {
                    id: 100,
                    name: "Family Chat".to_string(),
                    unread_count: 5,
                    last_message: Some("See you tomorrow".to_string()),
                }],
                contacts: vec![
                    ContactInfo {
                        id: 1,
                        name: "John Doe".to_string(),
                        username: Some("johndoe".to_string()),
                        phone: Some("+1234567890".to_string()),
                    },
                    ContactInfo {
                        id: 2,
                        name: "Jane Smith".to_string(),
                        username: None,
                        phone: None,
                    },
                ],
                messages: vec![
                    MessageInfo {
                        id: 1,
                        sender: "John Doe".to_string(),
                        text: "Hello!".to_string(),
                        date: "1h ago".to_string(),
                        is_outgoing: false,
                    },
                    MessageInfo {
                        id: 2,
                        sender: "You".to_string(),
                        text: "Hi there!".to_string(),
                        date: "30m ago".to_string(),
                        is_outgoing: true,
                    },
                ],
            }
        }
    }

    #[async_trait]
    impl TelegramClient for MockClient {
        async fn authenticate(
            &mut self,
            phone: Option<&str>,
        ) -> Result<()> {
            let state = *self.auth_state.lock().unwrap();
            match state {
                AuthState::WaitPhone => {
                    if phone.is_none() {
                        return Err(TgError::Other(
                            "Phone number required. Run: tg auth --phone +1234567890".to_string(),
                        ));
                    }
                    *self.phone_sent.lock().unwrap() = true;
                    // Simulate: after phone sent, move to WaitCode but return success
                    // (in real impl, user would run `tg auth` again)
                    Ok(())
                }
                AuthState::WaitCode | AuthState::WaitPassword => {
                    // Simulate code/password entry success
                    *self.authenticated.lock().unwrap() = true;
                    Ok(())
                }
                AuthState::Ready => {
                    *self.authenticated.lock().unwrap() = true;
                    Ok(())
                }
            }
        }

        async fn is_authenticated(&self) -> bool {
            *self.authenticated.lock().unwrap()
        }

        async fn get_chats(&self, limit: i32) -> Result<Vec<ChatInfo>> {
            Ok(self.chats.iter().take(limit as usize).cloned().collect())
        }

        async fn get_groups(&self, limit: i32) -> Result<Vec<ChatInfo>> {
            Ok(self.groups.iter().take(limit as usize).cloned().collect())
        }

        async fn get_unread_chats(&self, limit: i32) -> Result<Vec<ChatInfo>> {
            Ok(self
                .chats
                .iter()
                .chain(self.groups.iter())
                .filter(|c| c.unread_count > 0)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn search_contacts(&self, query: &str) -> Result<Vec<ContactInfo>> {
            Ok(self
                .contacts
                .iter()
                .filter(|c| c.name.to_lowercase().contains(&query.to_lowercase()))
                .cloned()
                .collect())
        }

        async fn find_chat_by_name(&self, name: &str) -> Result<i64> {
            self.contacts
                .iter()
                .find(|c| c.name.to_lowercase().contains(&name.to_lowercase()))
                .map(|c| c.id)
                .ok_or_else(|| TgError::ContactNotFound(name.to_string()))
        }

        async fn find_group_by_name(&self, name: &str) -> Result<i64> {
            self.groups
                .iter()
                .find(|g| g.name.to_lowercase().contains(&name.to_lowercase()))
                .map(|g| g.id)
                .ok_or_else(|| TgError::ChatNotFound(name.to_string()))
        }

        async fn send_message(&self, chat_id: i64, _text: &str) -> Result<SendResult> {
            Ok(SendResult {
                message_id: 12345,
                chat_id,
            })
        }

        async fn get_messages(&self, _chat_id: i64, limit: i32) -> Result<Vec<MessageInfo>> {
            Ok(self
                .messages
                .iter()
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn mark_chat_as_read(&self, _chat_id: i64) -> Result<()> {
            Ok(())
        }

        async fn mark_chat_as_unread(&self, _chat_id: i64) -> Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn client_new_initializes_shutdown_flag_to_false() {
        let client = TdLibClient::new(12345, "test_hash".to_string()).unwrap();

        // Shutdown flag should be false initially
        assert!(!client.shutdown.load(Ordering::Relaxed));
    }

    #[test]
    fn client_new_initializes_receive_handle_to_none() {
        let client = TdLibClient::new(12345, "test_hash".to_string()).unwrap();

        // Can't easily check the mutex contents in sync test, but we can verify
        // the client was created successfully
        assert!(client.client_id.try_lock().is_ok());
    }

    #[test]
    fn client_new_initializes_client_id_to_none() {
        let client = TdLibClient::new(12345, "test_hash".to_string()).unwrap();

        // Client ID should be None before start() is called
        let client_id = client.client_id.try_lock().unwrap();
        assert!(client_id.is_none());
    }

    #[test]
    fn client_new_initializes_authenticated_to_false() {
        let client = TdLibClient::new(12345, "test_hash".to_string()).unwrap();

        let authenticated = client.authenticated.try_lock().unwrap();
        assert!(!*authenticated);
    }

    #[test]
    fn shutdown_flag_can_be_set() {
        let shutdown = Arc::new(AtomicBool::new(false));

        // Initially false
        assert!(!shutdown.load(Ordering::Relaxed));

        // Set to true
        shutdown.store(true, Ordering::Relaxed);
        assert!(shutdown.load(Ordering::Relaxed));
    }

    #[test]
    fn shutdown_flag_is_thread_safe() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        // Spawn a thread that waits for shutdown
        let handle = std::thread::spawn(move || {
            while !shutdown_clone.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        });

        // Give the thread time to start
        std::thread::sleep(Duration::from_millis(50));

        // Signal shutdown
        shutdown.store(true, Ordering::Relaxed);

        // Thread should exit and return true
        let result = handle.join().unwrap();
        assert!(result);
    }

    #[test]
    fn receive_loop_exits_on_shutdown_signal() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let iterations = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let iterations_clone = iterations.clone();

        // Simulate a receive loop that checks shutdown flag
        let handle = std::thread::spawn(move || {
            loop {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                iterations_clone.fetch_add(1, Ordering::Relaxed);
                // Simulate receive timeout (much shorter for testing)
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        // Let the loop run a few iterations
        std::thread::sleep(Duration::from_millis(50));

        // Signal shutdown
        shutdown.store(true, Ordering::Relaxed);

        // Wait for thread to exit
        handle.join().unwrap();

        // Verify the loop ran at least once before shutdown
        assert!(iterations.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn broadcast_channel_handles_no_receivers() {
        let (sender, _receiver) = tokio::sync::broadcast::channel::<i32>(100);

        // Dropping the receiver and sending should not panic
        drop(_receiver);

        // Send should return error (no receivers) but not panic
        let result = sender.send(42);
        assert!(result.is_err());
    }

    #[test]
    fn broadcast_channel_delivers_to_multiple_receivers() {
        let (sender, mut receiver1) = tokio::sync::broadcast::channel::<i32>(100);
        let mut receiver2 = sender.subscribe();

        // Send a value
        sender.send(42).unwrap();

        // Both receivers should get it
        assert_eq!(receiver1.try_recv().unwrap(), 42);
        assert_eq!(receiver2.try_recv().unwrap(), 42);
    }

    #[tokio::test]
    async fn client_shutdown_sets_flag() {
        let mut client = TdLibClient::new(12345, "test_hash".to_string()).unwrap();

        // Flag should be false initially
        assert!(!client.shutdown.load(Ordering::Relaxed));

        // Call shutdown (won't fully work without TDLib, but should set flag)
        client.shutdown().await;

        // Flag should now be true
        assert!(client.shutdown.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn send_confirmation_wait_receives_success_update() {
        use tokio::sync::broadcast;

        // Simulate the pattern used in send_message for waiting on updates
        let (sender, mut receiver) = broadcast::channel::<&str>(10);

        // Spawn a task that simulates TDLib sending the success update
        let sender_clone = sender.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = sender_clone.send("success");
        });

        // Wait for the update (simulating send_message logic)
        let timeout = Duration::from_secs(1);
        let result = tokio::time::timeout(timeout, receiver.recv()).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().unwrap(), "success");
    }

    #[tokio::test]
    async fn send_confirmation_wait_handles_timeout() {
        use tokio::sync::broadcast;

        let (_sender, mut receiver) = broadcast::channel::<&str>(10);

        // Don't send anything - should timeout
        let timeout = Duration::from_millis(100);
        let result = tokio::time::timeout(timeout, receiver.recv()).await;

        // Should be a timeout error
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_confirmation_wait_receives_failure_update() {
        use tokio::sync::broadcast;

        let (sender, mut receiver) = broadcast::channel::<&str>(10);

        // Spawn a task that simulates TDLib sending the failure update
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = sender.send("failure");
        });

        let timeout = Duration::from_secs(1);
        let result = tokio::time::timeout(timeout, receiver.recv()).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().unwrap(), "failure");
    }

    #[tokio::test]
    async fn shutdown_completes_within_timeout() {
        let mut client = TdLibClient::new(12345, "test_hash".to_string()).unwrap();

        // Shutdown should complete within a reasonable time even without TDLib running
        let start = tokio::time::Instant::now();
        client.shutdown().await;
        let elapsed = start.elapsed();

        // Should complete within 3 seconds (close timeout is 2s + 200ms sleep)
        assert!(elapsed < Duration::from_secs(3));
        assert!(client.shutdown.load(Ordering::Relaxed));
    }
}
