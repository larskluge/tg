use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::{Result, TgError};
use crate::output::{ChatInfo, ContactInfo, MessageInfo, SendResult};

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
}

impl TdLibClient {
    pub fn new(api_id: i32, api_hash: String) -> Result<Self> {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tg");

        std::fs::create_dir_all(&data_dir)?;

        Ok(Self {
            client_id: Arc::new(Mutex::new(None)),
            api_id,
            api_hash,
            data_dir,
            authenticated: Arc::new(Mutex::new(false)),
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        let client_id = tdlib_rs::create_client();
        *self.client_id.lock().await = Some(client_id);

        // Set TDLib parameters
        self.set_tdlib_parameters().await?;

        // Wait for authorization state
        self.wait_for_ready().await
    }

    async fn set_tdlib_parameters(&self) -> Result<()> {
        let client_id = self.get_client_id().await?;

        tdlib_rs::functions::set_tdlib_parameters(
            false, // use_test_dc
            self.data_dir.join("db").to_string_lossy().to_string(),
            self.data_dir.join("files").to_string_lossy().to_string(),
            String::new(), // database_encryption_key
            true,  // use_file_database
            true,  // use_chat_info_database
            true,  // use_message_database
            false, // use_secret_chats
            self.api_id,
            self.api_hash.clone(),
            "en".to_string(),    // system_language_code
            "CLI".to_string(),   // device_model
            "1.0".to_string(),   // system_version
            env!("CARGO_PKG_VERSION").to_string(), // application_version
            client_id,
        )
        .await
        .map_err(|e| TgError::TdLib(e.message))?;

        Ok(())
    }

    async fn wait_for_ready(&self) -> Result<()> {
        use tdlib_rs::enums::AuthorizationState;

        loop {
            if let Some((tdlib_rs::enums::Update::AuthorizationState(state), _)) =
                tdlib_rs::receive()
            {
                match state.authorization_state {
                    AuthorizationState::Ready => {
                        *self.authenticated.lock().await = true;
                        return Ok(());
                    }
                    AuthorizationState::WaitPhoneNumber
                    | AuthorizationState::WaitCode(_)
                    | AuthorizationState::WaitPassword(_) => {
                        // Not yet authenticated, caller should use authenticate()
                        return Ok(());
                    }
                    AuthorizationState::Closed => {
                        return Err(TgError::AuthFailed("Session closed".to_string()));
                    }
                    _ => {}
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

        // Initialize client without waiting for ready state
        if self.client_id.lock().await.is_none() {
            let client_id = tdlib_rs::create_client();
            *self.client_id.lock().await = Some(client_id);
            self.set_tdlib_parameters().await?;
        }

        let client_id = self.get_client_id().await?;

        loop {
            if let Some((tdlib_rs::enums::Update::AuthorizationState(state), _)) =
                tdlib_rs::receive()
            {
                match state.authorization_state {
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
                        // After sending phone, wait for next state then exit
                    }
                    AuthorizationState::WaitCode(_) => {
                        if phone.is_some() {
                            // Phone was just sent, exit so user can run `tg auth` to enter code
                            println!("Verification code sent to your Telegram app.");
                            println!("Run `tg auth` to enter the code.");
                            return Ok(());
                        }
                        // No phone provided, prompt for code
                        print!("Enter verification code: ");
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
                        println!("Already authenticated.");
                        return Ok(());
                    }
                    AuthorizationState::Closed => {
                        return Err(TgError::AuthFailed("Session closed".to_string()));
                    }
                    _ => {}
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
                result.push(ContactInfo {
                    id: user_id,
                    name: get_user_full_name(&user),
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
        use tdlib_rs::enums::InputMessageContent;
        use tdlib_rs::types::{FormattedText, InputMessageText};

        let client_id = self.get_client_id().await?;

        let content = InputMessageContent::InputMessageText(InputMessageText {
            text: FormattedText {
                text: text.to_string(),
                entities: vec![],
            },
            link_preview_options: None,
            clear_draft: true,
        });

        let message_enum = tdlib_rs::functions::send_message(
            chat_id, 0, None, None, content, client_id,
        )
        .await
        .map_err(|e| TgError::TdLib(e.message))?;

        let message = unwrap_message(message_enum);
        Ok(SendResult {
            message_id: message.id,
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
                    },
                    ContactInfo {
                        id: 2,
                        name: "Jane Smith".to_string(),
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
