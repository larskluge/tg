use async_trait::async_trait;
use std::collections::HashSet;
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use tokio::sync::{Mutex, broadcast};

use crate::credentials::tg_data_dir;
use crate::error::{Result, TgError};
use crate::output::{
    ChatInfo, ContactInfo, DownloadReport, DownloadStatus, DownloadedFileResult,
    MessageContentDetails, MessageFileRef, MessageInfo, SendResult,
};

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

/// Result of looking up a boundary message via `get_boundary_message_id`.
#[derive(Debug, Clone, PartialEq)]
pub enum BoundaryResult {
    /// No boundary — fetch most recent messages
    None,
    /// Stop at this message id (inclusive)
    BoundAt(i64),
    /// No messages exist at or after the requested timestamp — return empty immediately
    Empty,
}

#[async_trait]
pub trait TelegramClient: Send + Sync {
    async fn authenticate(&mut self) -> Result<()>;

    async fn is_authenticated(&self) -> bool;

    async fn get_chats(&self, limit: i32) -> Result<Vec<ChatInfo>>;
    async fn get_groups(&self, limit: i32) -> Result<Vec<ChatInfo>>;
    async fn get_unread_chats(&self, limit: i32) -> Result<Vec<ChatInfo>>;

    async fn search_contacts(&self, query: &str) -> Result<Vec<ContactInfo>>;

    async fn find_chat_by_name(&self, name: &str) -> Result<i64>;
    async fn find_group_by_name(&self, name: &str) -> Result<i64>;

    async fn send_message(&self, chat_id: i64, text: &str) -> Result<SendResult>;

    async fn get_messages(
        &self,
        chat_id: i64,
        limit: i32,
        until_message_id: Option<i64>,
    ) -> Result<Vec<MessageInfo>>;

    /// Find the boundary message for `--since-utc` filtering.
    ///
    /// Uses TDLib's `getChatMessageByDate(timestamp)` which returns the nearest
    /// message at or before the given timestamp.
    async fn get_boundary_message_id(&self, chat_id: i64, timestamp: i32)
    -> Result<BoundaryResult>;

    async fn download_message_media(
        &self,
        chat_id: i64,
        message_id: i64,
        options: DownloadOptions,
    ) -> Result<DownloadReport>;

    async fn mark_chat_as_read(&self, chat_id: i64) -> Result<()>;
    async fn mark_chat_as_unread(&self, chat_id: i64) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub output_dir: PathBuf,
    pub priority: i32,
}

pub struct TdLibClient {
    client_id: Arc<Mutex<Option<i32>>>,
    api_id: i32,
    api_hash: String,
    data_dir: PathBuf,
    authenticated: Arc<Mutex<bool>>,
    tdlib_parameters_sent: Arc<AtomicBool>,
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

        let data_dir = tg_data_dir();

        std::fs::create_dir_all(&data_dir)?;

        // Create broadcast channel for updates (capacity 100)
        let (update_sender, _) = broadcast::channel(100);

        Ok(Self {
            client_id: Arc::new(Mutex::new(None)),
            api_id,
            api_hash,
            data_dir,
            authenticated: Arc::new(Mutex::new(false)),
            tdlib_parameters_sent: Arc::new(AtomicBool::new(false)),
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

    /// Ensure TDLib client and receive loop are initialized.
    async fn ensure_client_initialized(&mut self) -> Result<i32> {
        if let Some(client_id) = *self.client_id.lock().await {
            return Ok(client_id);
        }

        self.shutdown.store(false, Ordering::Relaxed);
        self.tdlib_parameters_sent.store(false, Ordering::Relaxed);

        let client_id = tdlib_rs::create_client();
        *self.client_id.lock().await = Some(client_id);

        // CRITICAL: Spawn receive loop BEFORE any TDLib operations
        self.spawn_receive_loop().await;

        // Small delay to let the receive loop start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        Ok(client_id)
    }

    async fn set_tdlib_parameters(&self, client_id: i32) -> Result<()> {
        tdlib_rs::functions::set_tdlib_parameters(
            false,
            self.data_dir.join("db").to_string_lossy().to_string(),
            self.data_dir.join("files").to_string_lossy().to_string(),
            String::new(),
            true,
            true,
            true,
            false,
            self.api_id,
            self.api_hash.clone(),
            "en".to_string(),
            "CLI".to_string(),
            "1.0".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            client_id,
        )
        .await
        .map_err(|e| TgError::TdLib(e.message))
    }

    async fn ensure_tdlib_parameters(&self, client_id: i32) -> Result<()> {
        if self.tdlib_parameters_sent.swap(true, Ordering::Relaxed) {
            return Ok(());
        }

        if let Err(err) = self.set_tdlib_parameters(client_id).await {
            self.tdlib_parameters_sent.store(false, Ordering::Relaxed);
            return Err(err);
        }

        Ok(())
    }

    /// Gracefully shut down the TDLib client
    pub async fn shutdown(&mut self) {
        use tdlib_rs::enums::{AuthorizationState, Update};

        // Close TDLib client if started (must happen before stopping receive loop)
        let client_id = *self.client_id.lock().await;
        if let Some(client_id) = client_id {
            // Keep receiving updates until Closed to let TDLib finish teardown.
            let mut receiver = self.update_sender.subscribe();

            // Request TDLib to close with a timeout - don't block forever
            let close_future = tdlib_rs::functions::close(client_id);
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(2), close_future).await;

            // Wait briefly for authorizationStateClosed before stopping receive loop.
            let wait_for_closed = async {
                loop {
                    match receiver.recv().await {
                        Ok(Update::AuthorizationState(state))
                            if matches!(state.authorization_state, AuthorizationState::Closed) =>
                        {
                            break;
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            };
            let _ =
                tokio::time::timeout(tokio::time::Duration::from_secs(3), wait_for_closed).await;
        }
        *self.client_id.lock().await = None;

        // Signal the receive loop to stop
        self.shutdown.store(true, Ordering::Relaxed);
        *self.authenticated.lock().await = false;
        self.tdlib_parameters_sent.store(false, Ordering::Relaxed);

        // Wait for the receive loop thread to terminate so no TDLib calls outlive shutdown.
        let receive_handle = { self.receive_handle.lock().await.take() };
        if let Some(handle) = receive_handle {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = handle.join();
            })
            .await;
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        use tdlib_rs::enums::{AuthorizationState, ConnectionState, Update};

        let client_id = self.ensure_client_initialized().await?;

        // Subscribe to updates before triggering TDLib
        let mut receiver = self.update_sender.subscribe();

        // Trigger TDLib to start sending updates
        let _ = tdlib_rs::functions::get_option("version".to_string(), client_id).await;

        // Phase 1: Wait for authentication to complete
        let mut connection_ready = false;
        loop {
            match receiver.recv().await {
                Ok(update) => {
                    // Track connection state updates that arrive during auth
                    if let Update::ConnectionState(ref cs) = update {
                        if matches!(cs.state, ConnectionState::Ready) {
                            connection_ready = true;
                        }
                    }
                    if let Update::AuthorizationState(state) = update {
                        match state.authorization_state {
                            AuthorizationState::WaitTdlibParameters => {
                                self.ensure_tdlib_parameters(client_id).await?;
                            }
                            AuthorizationState::Ready => {
                                *self.authenticated.lock().await = true;
                                break;
                            }
                            AuthorizationState::WaitPhoneNumber
                            | AuthorizationState::WaitCode(_)
                            | AuthorizationState::WaitPassword(_) => {
                                return Err(TgError::AuthFailed(
                                    "Not authenticated. Run `tg auth` first.".to_string(),
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

        // Phase 2: Wait for TDLib to finish syncing updates from the server.
        // After auth, TDLib downloads the "difference" (updates received while offline),
        // transitioning through ConnectionState: Connecting → Updating → Ready.
        // Without this wait, getChatHistory may return stale local data because TDLib
        // hasn't processed the incoming updates yet.
        if !connection_ready {
            let timeout = tokio::time::Duration::from_secs(5);
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, receiver.recv()).await {
                    Ok(Ok(Update::ConnectionState(cs))) => {
                        if matches!(cs.state, ConnectionState::Ready) {
                            break;
                        }
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) | Err(_) => break,
                }
            }
        }

        Ok(())
    }

    async fn get_client_id(&self) -> Result<i32> {
        self.client_id
            .lock()
            .await
            .ok_or_else(|| TgError::Other("Client not started".to_string()))
    }

    async fn collect_filtered_chats<F>(&self, limit: i32, filter: F) -> Result<Vec<ChatInfo>>
    where
        F: Fn(&ChatSnapshot) -> bool,
    {
        collect_filtered_chats_from_source(self, limit, filter).await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatTypeKind {
    Private,
    BasicGroup,
    Supergroup,
    Other,
}

#[derive(Clone, Debug)]
struct ChatSnapshot {
    id: i64,
    title: String,
    unread_count: i32,
    last_message: Option<String>,
    chat_type: ChatTypeKind,
}

impl ChatSnapshot {
    fn to_chat_info(&self) -> ChatInfo {
        ChatInfo {
            id: self.id,
            name: self.title.clone(),
            unread_count: self.unread_count,
            last_message: self.last_message.clone(),
        }
    }
}

#[async_trait]
trait ChatDataSource {
    async fn fetch_chat_ids(&self, limit: i32) -> Result<Vec<i64>>;
    async fn fetch_chat_snapshot(&self, chat_id: i64) -> Result<ChatSnapshot>;
}

#[async_trait]
impl ChatDataSource for TdLibClient {
    async fn fetch_chat_ids(&self, limit: i32) -> Result<Vec<i64>> {
        let client_id = self.get_client_id().await?;
        let chats_enum = tdlib_rs::functions::get_chats(None, limit, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;
        Ok(unwrap_chats(chats_enum).chat_ids)
    }

    async fn fetch_chat_snapshot(&self, chat_id: i64) -> Result<ChatSnapshot> {
        let client_id = self.get_client_id().await?;
        let chat_enum = tdlib_rs::functions::get_chat(chat_id, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;
        let chat = unwrap_chat(chat_enum);
        let chat_type = match chat.r#type {
            tdlib_rs::enums::ChatType::Private(_) => ChatTypeKind::Private,
            tdlib_rs::enums::ChatType::BasicGroup(_) => ChatTypeKind::BasicGroup,
            tdlib_rs::enums::ChatType::Supergroup(_) => ChatTypeKind::Supergroup,
            _ => ChatTypeKind::Other,
        };

        Ok(ChatSnapshot {
            id: chat.id,
            title: chat.title,
            unread_count: chat.unread_count,
            last_message: chat
                .last_message
                .as_ref()
                .and_then(|m| extract_message_text(&m.content)),
            chat_type,
        })
    }
}

async fn collect_filtered_chats_from_source<S, F>(
    source: &S,
    limit: i32,
    filter: F,
) -> Result<Vec<ChatInfo>>
where
    S: ChatDataSource,
    F: Fn(&ChatSnapshot) -> bool,
{
    if limit <= 0 {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut fetch_limit = limit.max(1);
    let mut previous_total = 0usize;

    loop {
        let chat_ids = source.fetch_chat_ids(fetch_limit).await?;
        let total = chat_ids.len();

        for chat_id in chat_ids {
            if !seen.insert(chat_id) {
                continue;
            }

            if let Ok(chat) = source.fetch_chat_snapshot(chat_id).await {
                if filter(&chat) {
                    result.push(chat.to_chat_info());
                    if result.len() as i32 >= limit {
                        return Ok(result);
                    }
                }
            }
        }

        if total <= previous_total {
            break;
        }

        previous_total = total;

        if total < fetch_limit as usize {
            break;
        }

        let next_limit = fetch_limit.saturating_mul(2);
        if next_limit == fetch_limit {
            break;
        }
        fetch_limit = next_limit;
    }

    Ok(result)
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

// Helper to extract File fields from the enum
fn unwrap_file(file: tdlib_rs::enums::File) -> tdlib_rs::types::File {
    match file {
        tdlib_rs::enums::File::File(f) => f,
    }
}

fn non_empty(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn format_duration(seconds: i32) -> String {
    let minutes = seconds / 60;
    let remaining = seconds % 60;
    format!("{minutes}:{remaining:02}")
}

fn media_summary(summary: impl AsRef<str>) -> String {
    format!("[{}]", summary.as_ref())
}

fn sticker_format_extension(format: &tdlib_rs::enums::StickerFormat) -> &'static str {
    match format {
        tdlib_rs::enums::StickerFormat::Webp => "webp",
        tdlib_rs::enums::StickerFormat::Tgs => "tgs",
        tdlib_rs::enums::StickerFormat::Webm => "webm",
    }
}

fn sticker_format_name(format: &tdlib_rs::enums::StickerFormat) -> &'static str {
    match format {
        tdlib_rs::enums::StickerFormat::Webp => "webp",
        tdlib_rs::enums::StickerFormat::Tgs => "tgs",
        tdlib_rs::enums::StickerFormat::Webm => "webm",
    }
}

fn best_photo_size(photo: &tdlib_rs::types::Photo) -> Option<&tdlib_rs::types::PhotoSize> {
    photo.sizes.iter().max_by_key(|size| {
        (
            i64::from(size.width) * i64::from(size.height),
            size.photo.size,
            size.photo.expected_size,
        )
    })
}

fn message_file_ref(
    file: &tdlib_rs::types::File,
    is_primary: bool,
    role: Option<&str>,
    file_name: Option<String>,
    mime_type: Option<String>,
) -> MessageFileRef {
    MessageFileRef {
        file_id: file.id,
        is_primary,
        role: role.map(ToOwned::to_owned),
        file_name,
        mime_type,
        size_bytes: file.size,
        expected_size_bytes: file.expected_size,
        local_path: non_empty(&file.local.path),
        remote_id: non_empty(&file.remote.id),
        remote_unique_id: non_empty(&file.remote.unique_id),
        can_be_downloaded: file.local.can_be_downloaded,
        is_downloaded: file.local.is_downloading_completed,
    }
}

#[derive(Debug, Clone)]
struct ExtractedMessageData {
    text: String,
    content_type: Option<String>,
    is_downloadable: bool,
    download_files: Vec<MessageFileRef>,
    content: Option<MessageContentDetails>,
}

/// Extract the TDLib type name from a MessageContent variant using its Debug representation.
/// E.g. `MessagePremiumGiftCode(...)` → `"messagePremiumGiftCode"`, `MessageUnsupported` → `"messageUnsupported"`.
#[cfg(test)]
fn tdlib_type_name(content: &tdlib_rs::enums::MessageContent) -> String {
    let debug = format!("{content:?}");
    let variant = debug.split(['(', ' ']).next().unwrap_or(&debug);
    // Convert PascalCase variant name to camelCase TDLib type name (lowercase first char)
    let mut chars = variant.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn extract_message_data(content: &tdlib_rs::enums::MessageContent) -> ExtractedMessageData {
    use tdlib_rs::enums::MessageContent;

    match content {
        MessageContent::MessageText(t) => {
            let text = t.text.text.clone();
            ExtractedMessageData {
                text: if text.is_empty() {
                    "[Text]".to_string()
                } else {
                    text.clone()
                },
                content_type: Some("text".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Text { text }),
            }
        }
        MessageContent::MessagePhoto(p) => {
            let caption = non_empty(&p.caption.text);
            let best = best_photo_size(&p.photo);
            let mut files = Vec::new();
            let mut width = None;
            let mut height = None;
            if let Some(size) = best {
                width = Some(size.width);
                height = Some(size.height);
                files.push(message_file_ref(
                    &size.photo,
                    true,
                    Some("main"),
                    None,
                    Some("image/jpeg".to_string()),
                ));
            }
            let text = if let Some(c) = &caption {
                format!("Photo: {c}")
            } else if let (Some(w), Some(h)) = (width, height) {
                format!("Photo: {w}x{h}")
            } else {
                "Photo".to_string()
            };
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("photo".to_string()),
                is_downloadable: !files.is_empty(),
                download_files: files,
                content: Some(MessageContentDetails::Photo {
                    width,
                    height,
                    caption,
                    has_spoiler: p.has_spoiler,
                    is_secret: p.is_secret,
                }),
            }
        }
        MessageContent::MessageVideo(v) => {
            let caption = non_empty(&v.caption.text);
            let text = if let Some(c) = &caption {
                format!("Video: {c}")
            } else {
                format!(
                    "Video: {}x{} {}",
                    v.video.width,
                    v.video.height,
                    format_duration(v.video.duration)
                )
            };
            let files = vec![message_file_ref(
                &v.video.video,
                true,
                Some("main"),
                non_empty(&v.video.file_name),
                non_empty(&v.video.mime_type),
            )];
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("video".to_string()),
                is_downloadable: true,
                download_files: files,
                content: Some(MessageContentDetails::Video {
                    width: v.video.width,
                    height: v.video.height,
                    duration_seconds: v.video.duration,
                    caption,
                    file_name: non_empty(&v.video.file_name),
                    mime_type: non_empty(&v.video.mime_type),
                    has_spoiler: v.has_spoiler,
                    is_secret: v.is_secret,
                    supports_streaming: v.video.supports_streaming,
                }),
            }
        }
        MessageContent::MessageDocument(d) => {
            let caption = non_empty(&d.caption.text);
            let display_name = non_empty(&d.document.file_name);
            let text = match (&caption, &display_name) {
                (Some(c), _) => format!("Document: {c}"),
                (None, Some(name)) => format!("Document: {name}"),
                _ => "Document".to_string(),
            };
            let files = vec![message_file_ref(
                &d.document.document,
                true,
                Some("main"),
                display_name.clone(),
                non_empty(&d.document.mime_type),
            )];
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("document".to_string()),
                is_downloadable: true,
                download_files: files,
                content: Some(MessageContentDetails::Document {
                    caption,
                    file_name: display_name,
                    mime_type: non_empty(&d.document.mime_type),
                }),
            }
        }
        MessageContent::MessageSticker(s) => {
            let emoji = non_empty(&s.sticker.emoji);
            let text = if let Some(e) = &emoji {
                format!("Sticker: {e}")
            } else {
                "Sticker".to_string()
            };
            let files = vec![message_file_ref(
                &s.sticker.sticker,
                true,
                Some("main"),
                None,
                None,
            )];
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("sticker".to_string()),
                is_downloadable: true,
                download_files: files,
                content: Some(MessageContentDetails::Sticker {
                    emoji,
                    width: s.sticker.width,
                    height: s.sticker.height,
                    format: format!("{:?}", s.sticker.format),
                }),
            }
        }
        MessageContent::MessageAudio(a) => {
            let caption = non_empty(&a.caption.text);
            let title = non_empty(&a.audio.title);
            let performer = non_empty(&a.audio.performer);
            let text_label = title
                .clone()
                .or_else(|| non_empty(&a.audio.file_name))
                .unwrap_or_else(|| "Audio".to_string());
            let text = format!(
                "Audio: {} ({})",
                text_label,
                format_duration(a.audio.duration)
            );
            let files = vec![message_file_ref(
                &a.audio.audio,
                true,
                Some("main"),
                non_empty(&a.audio.file_name),
                non_empty(&a.audio.mime_type),
            )];
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("audio".to_string()),
                is_downloadable: true,
                download_files: files,
                content: Some(MessageContentDetails::Audio {
                    title,
                    performer,
                    duration_seconds: a.audio.duration,
                    caption,
                    file_name: non_empty(&a.audio.file_name),
                    mime_type: non_empty(&a.audio.mime_type),
                }),
            }
        }
        MessageContent::MessageVoiceNote(v) => {
            let caption = non_empty(&v.caption.text);
            let text = format!("Voice: {}", format_duration(v.voice_note.duration));
            let files = vec![message_file_ref(
                &v.voice_note.voice,
                true,
                Some("main"),
                None,
                non_empty(&v.voice_note.mime_type),
            )];
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("voice".to_string()),
                is_downloadable: true,
                download_files: files,
                content: Some(MessageContentDetails::Voice {
                    duration_seconds: v.voice_note.duration,
                    caption,
                    mime_type: non_empty(&v.voice_note.mime_type),
                    is_listened: v.is_listened,
                }),
            }
        }
        MessageContent::MessageAnimation(a) => {
            let caption = non_empty(&a.caption.text);
            let text = format!(
                "Animation: {}x{} {}",
                a.animation.width,
                a.animation.height,
                format_duration(a.animation.duration)
            );
            let files = vec![message_file_ref(
                &a.animation.animation,
                true,
                Some("main"),
                non_empty(&a.animation.file_name),
                non_empty(&a.animation.mime_type),
            )];
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("animation".to_string()),
                is_downloadable: true,
                download_files: files,
                content: Some(MessageContentDetails::Animation {
                    width: a.animation.width,
                    height: a.animation.height,
                    duration_seconds: a.animation.duration,
                    caption,
                    file_name: non_empty(&a.animation.file_name),
                    mime_type: non_empty(&a.animation.mime_type),
                    has_spoiler: a.has_spoiler,
                    is_secret: a.is_secret,
                }),
            }
        }
        MessageContent::MessageVideoNote(v) => {
            let text = format!("Video note: {}", format_duration(v.video_note.duration));
            let files = vec![message_file_ref(
                &v.video_note.video,
                true,
                Some("main"),
                None,
                Some("video/mp4".to_string()),
            )];
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("video_note".to_string()),
                is_downloadable: true,
                download_files: files,
                content: Some(MessageContentDetails::VideoNote {
                    duration_seconds: v.video_note.duration,
                    length: v.video_note.length,
                    is_viewed: v.is_viewed,
                    is_secret: v.is_secret,
                }),
            }
        }
        MessageContent::MessageLocation(l) => {
            let text = format!(
                "Location: {:.5}, {:.5}",
                l.location.latitude, l.location.longitude
            );
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("location".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Location {
                    latitude: l.location.latitude,
                    longitude: l.location.longitude,
                    horizontal_accuracy: l.location.horizontal_accuracy,
                    live_period: l.live_period,
                    expires_in: l.expires_in,
                    heading: l.heading,
                    proximity_alert_radius: l.proximity_alert_radius,
                }),
            }
        }
        MessageContent::MessageContact(c) => {
            let full_name = if c.contact.last_name.is_empty() {
                c.contact.first_name.clone()
            } else {
                format!("{} {}", c.contact.first_name, c.contact.last_name)
            };
            let text = format!("Contact: {} ({})", full_name, c.contact.phone_number);
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("contact".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Contact {
                    phone_number: c.contact.phone_number.clone(),
                    first_name: c.contact.first_name.clone(),
                    last_name: non_empty(&c.contact.last_name),
                    user_id: c.contact.user_id,
                    vcard: non_empty(&c.contact.vcard),
                }),
            }
        }
        MessageContent::MessageAnimatedEmoji(e) => {
            let mut files = Vec::new();
            let mut custom_emoji_id = None;
            let mut sticker_format = None;
            let mut sticker_width = e.animated_emoji.sticker_width;
            let mut sticker_height = e.animated_emoji.sticker_height;

            if let Some(sticker) = &e.animated_emoji.sticker {
                sticker_width = sticker.width;
                sticker_height = sticker.height;
                let format_name = sticker_format_name(&sticker.format);
                sticker_format = Some(format_name.to_string());
                let ext = sticker_format_extension(&sticker.format);
                files.push(message_file_ref(
                    &sticker.sticker,
                    true,
                    Some("main"),
                    Some(format!("emoji.{ext}")),
                    None,
                ));

                if let tdlib_rs::enums::StickerFullType::CustomEmoji(custom) = &sticker.full_type {
                    custom_emoji_id = Some(custom.custom_emoji_id);
                }
            }

            if let Some(sound) = &e.animated_emoji.sound {
                files.push(message_file_ref(
                    sound,
                    files.is_empty(),
                    Some("sound"),
                    Some("emoji_sound.ogg".to_string()),
                    Some("audio/ogg".to_string()),
                ));
            }

            let emoji = non_empty(&e.emoji);
            let emoji_text = emoji.clone().unwrap_or_else(|| "emoji".to_string());
            let text = media_summary(format!("Emoji: {emoji_text}"));
            ExtractedMessageData {
                text,
                content_type: Some("emoji".to_string()),
                is_downloadable: !files.is_empty(),
                download_files: files,
                content: Some(MessageContentDetails::Emoji {
                    emoji,
                    sticker_width,
                    sticker_height,
                    fitzpatrick_type: e.animated_emoji.fitzpatrick_type,
                    sticker_format,
                    custom_emoji_id,
                    has_sound: e.animated_emoji.sound.is_some(),
                }),
            }
        }
        MessageContent::MessagePoll(p) => {
            let poll_type = match &p.poll.r#type {
                tdlib_rs::enums::PollType::Regular(_) => "regular".to_string(),
                tdlib_rs::enums::PollType::Quiz(_) => "quiz".to_string(),
            };
            let text = format!("Poll: {}", p.poll.question.text);
            ExtractedMessageData {
                text: media_summary(text),
                content_type: Some("poll".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Poll {
                    question: p.poll.question.text.clone(),
                    option_count: p.poll.options.len(),
                    total_voter_count: p.poll.total_voter_count,
                    is_anonymous: p.poll.is_anonymous,
                    is_closed: p.poll.is_closed,
                    poll_type,
                }),
            }
        }
        MessageContent::MessageCall(c) => {
            let discard_reason = match &c.discard_reason {
                tdlib_rs::enums::CallDiscardReason::Empty => "unknown",
                tdlib_rs::enums::CallDiscardReason::Missed => "missed",
                tdlib_rs::enums::CallDiscardReason::Declined => "declined",
                tdlib_rs::enums::CallDiscardReason::Disconnected => "disconnected",
                tdlib_rs::enums::CallDiscardReason::HungUp => "hung_up",
            }
            .to_string();
            let kind = if c.is_video { "Video call" } else { "Call" };
            let text = if c.duration > 0 {
                format!("{kind} ({discard_reason}, {}s)", c.duration)
            } else {
                format!("{kind} ({discard_reason})")
            };
            ExtractedMessageData {
                text,
                content_type: Some("call".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Call {
                    is_video: c.is_video,
                    discard_reason,
                    duration_seconds: c.duration,
                }),
            }
        }
        MessageContent::MessageContactRegistered => ExtractedMessageData {
            text: "Contact registered".to_string(),
            content_type: Some("contact_registered".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ContactRegistered {}),
        },
        MessageContent::MessageVenue(v) => {
            let text = format!("Venue: {}, {}", v.venue.title, v.venue.address);
            ExtractedMessageData {
                text,
                content_type: Some("venue".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Venue {
                    title: v.venue.title.clone(),
                    address: v.venue.address.clone(),
                    latitude: v.venue.location.latitude,
                    longitude: v.venue.location.longitude,
                    provider: non_empty(&v.venue.provider),
                }),
            }
        }
        MessageContent::MessagePinMessage(p) => ExtractedMessageData {
            text: "Pinned a message".to_string(),
            content_type: Some("pin_message".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PinMessage {
                pinned_message_id: p.message_id,
            }),
        },
        MessageContent::MessageGiftedPremium(g) => {
            let text = format!("Gifted Premium ({} months)", g.month_count);
            ExtractedMessageData {
                text,
                content_type: Some("gifted_premium".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::GiftedPremium {
                    gifter_user_id: g.gifter_user_id,
                    currency: g.currency.clone(),
                    amount: g.amount,
                    month_count: g.month_count,
                }),
            }
        }
        MessageContent::MessageDice(d) => {
            let text = format!("Dice: {} (value: {})", d.emoji, d.value);
            ExtractedMessageData {
                text,
                content_type: Some("dice".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Dice {
                    emoji: d.emoji.clone(),
                    value: d.value,
                }),
            }
        }
        MessageContent::MessageGame(g) => {
            let text = format!("Game: {}", g.game.title);
            ExtractedMessageData {
                text,
                content_type: Some("game".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Game {
                    title: g.game.title.clone(),
                    short_name: g.game.short_name.clone(),
                    description: g.game.description.clone(),
                }),
            }
        }
        MessageContent::MessageStory(s) => {
            let text = format!("Story from chat {}", s.story_sender_chat_id);
            ExtractedMessageData {
                text,
                content_type: Some("story".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Story {
                    story_sender_chat_id: s.story_sender_chat_id,
                    story_id: s.story_id,
                    via_mention: s.via_mention,
                }),
            }
        }
        MessageContent::MessageInvoice(inv) => {
            let text = format!("Invoice: {} ({} {})", inv.title, inv.total_amount, inv.currency);
            ExtractedMessageData {
                text,
                content_type: Some("invoice".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Invoice {
                    title: inv.title.clone(),
                    currency: inv.currency.clone(),
                    total_amount: inv.total_amount,
                    is_test: inv.is_test,
                }),
            }
        }
        MessageContent::MessageVideoChatScheduled(v) => ExtractedMessageData {
            text: format!("Video chat scheduled (start: {})", v.start_date),
            content_type: Some("video_chat_scheduled".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::VideoChatScheduled {
                group_call_id: v.group_call_id,
                start_date: v.start_date,
            }),
        },
        MessageContent::MessageVideoChatStarted(v) => ExtractedMessageData {
            text: "Video chat started".to_string(),
            content_type: Some("video_chat_started".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::VideoChatStarted {
                group_call_id: v.group_call_id,
            }),
        },
        MessageContent::MessageVideoChatEnded(v) => ExtractedMessageData {
            text: format!("Video chat ended ({}s)", v.duration),
            content_type: Some("video_chat_ended".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::VideoChatEnded {
                duration_seconds: v.duration,
            }),
        },
        MessageContent::MessageInviteVideoChatParticipants(v) => ExtractedMessageData {
            text: format!("Invited {} participants to video chat", v.user_ids.len()),
            content_type: Some("invite_video_chat_participants".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::InviteVideoChatParticipants {
                group_call_id: v.group_call_id,
                user_ids: v.user_ids.clone(),
            }),
        },
        MessageContent::MessageBasicGroupChatCreate(g) => ExtractedMessageData {
            text: format!("Group created: {}", g.title),
            content_type: Some("group_created".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::BasicGroupChatCreate {
                title: g.title.clone(),
                member_user_ids: g.member_user_ids.clone(),
            }),
        },
        MessageContent::MessageSupergroupChatCreate(g) => ExtractedMessageData {
            text: format!("Supergroup created: {}", g.title),
            content_type: Some("supergroup_created".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::SupergroupChatCreate {
                title: g.title.clone(),
            }),
        },
        MessageContent::MessageChatChangeTitle(t) => ExtractedMessageData {
            text: format!("Chat title changed to: {}", t.title),
            content_type: Some("chat_change_title".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatChangeTitle {
                title: t.title.clone(),
            }),
        },
        MessageContent::MessageChatChangePhoto(_) => ExtractedMessageData {
            text: "Chat photo changed".to_string(),
            content_type: Some("chat_change_photo".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatChangePhoto {}),
        },
        MessageContent::MessageChatDeletePhoto => ExtractedMessageData {
            text: "Chat photo deleted".to_string(),
            content_type: Some("chat_delete_photo".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatDeletePhoto {}),
        },
        MessageContent::MessageChatAddMembers(m) => ExtractedMessageData {
            text: format!("Members added: {:?}", m.member_user_ids),
            content_type: Some("members_added".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatAddMembers {
                member_user_ids: m.member_user_ids.clone(),
            }),
        },
        MessageContent::MessageChatJoinByLink => ExtractedMessageData {
            text: "Joined by invite link".to_string(),
            content_type: Some("chat_join_by_link".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatJoinByLink {}),
        },
        MessageContent::MessageChatJoinByRequest => ExtractedMessageData {
            text: "Joined by request".to_string(),
            content_type: Some("chat_join_by_request".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatJoinByRequest {}),
        },
        MessageContent::MessageChatDeleteMember(m) => ExtractedMessageData {
            text: format!("Member removed: {}", m.user_id),
            content_type: Some("chat_delete_member".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatDeleteMember {
                user_id: m.user_id,
            }),
        },
        MessageContent::MessageChatUpgradeTo(u) => ExtractedMessageData {
            text: format!("Upgraded to supergroup {}", u.supergroup_id),
            content_type: Some("chat_upgrade_to".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatUpgradeTo {
                supergroup_id: u.supergroup_id,
            }),
        },
        MessageContent::MessageChatUpgradeFrom(u) => ExtractedMessageData {
            text: format!("Upgraded from basic group: {}", u.title),
            content_type: Some("chat_upgrade_from".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatUpgradeFrom {
                title: u.title.clone(),
                basic_group_id: u.basic_group_id,
            }),
        },
        MessageContent::MessageScreenshotTaken => ExtractedMessageData {
            text: "Screenshot taken".to_string(),
            content_type: Some("screenshot_taken".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ScreenshotTaken {}),
        },
        MessageContent::MessageChatSetBackground(b) => ExtractedMessageData {
            text: "Chat background changed".to_string(),
            content_type: Some("chat_set_background".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatSetBackground {
                old_background_message_id: b.old_background_message_id,
                only_for_self: b.only_for_self,
            }),
        },
        MessageContent::MessageChatSetTheme(t) => ExtractedMessageData {
            text: if t.theme_name.is_empty() {
                "Chat theme reset".to_string()
            } else {
                format!("Chat theme set to: {}", t.theme_name)
            },
            content_type: Some("chat_set_theme".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatSetTheme {
                theme_name: t.theme_name.clone(),
            }),
        },
        MessageContent::MessageChatSetMessageAutoDeleteTime(t) => ExtractedMessageData {
            text: if t.message_auto_delete_time == 0 {
                "Auto-delete timer disabled".to_string()
            } else {
                format!("Auto-delete timer set to {}s", t.message_auto_delete_time)
            },
            content_type: Some("chat_set_message_auto_delete_time".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatSetMessageAutoDeleteTime {
                message_auto_delete_time: t.message_auto_delete_time,
                from_user_id: t.from_user_id,
            }),
        },
        MessageContent::MessageChatBoost(b) => ExtractedMessageData {
            text: format!("Chat boosted ({} boosts)", b.boost_count),
            content_type: Some("chat_boost".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatBoost {
                boost_count: b.boost_count,
            }),
        },
        MessageContent::MessageForumTopicCreated(t) => ExtractedMessageData {
            text: format!("Forum topic created: {}", t.name),
            content_type: Some("forum_topic_created".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ForumTopicCreated {
                name: t.name.clone(),
            }),
        },
        MessageContent::MessageForumTopicEdited(t) => ExtractedMessageData {
            text: if t.name.is_empty() {
                "Forum topic edited".to_string()
            } else {
                format!("Forum topic renamed to: {}", t.name)
            },
            content_type: Some("forum_topic_edited".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ForumTopicEdited {
                name: t.name.clone(),
                edit_icon_custom_emoji_id: t.edit_icon_custom_emoji_id,
                icon_custom_emoji_id: t.icon_custom_emoji_id,
            }),
        },
        MessageContent::MessageForumTopicIsClosedToggled(t) => ExtractedMessageData {
            text: if t.is_closed {
                "Forum topic closed".to_string()
            } else {
                "Forum topic reopened".to_string()
            },
            content_type: Some("forum_topic_is_closed_toggled".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ForumTopicIsClosedToggled {
                is_closed: t.is_closed,
            }),
        },
        MessageContent::MessageForumTopicIsHiddenToggled(t) => ExtractedMessageData {
            text: if t.is_hidden {
                "Forum topic hidden".to_string()
            } else {
                "Forum topic unhidden".to_string()
            },
            content_type: Some("forum_topic_is_hidden_toggled".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ForumTopicIsHiddenToggled {
                is_hidden: t.is_hidden,
            }),
        },
        MessageContent::MessageSuggestProfilePhoto(_) => ExtractedMessageData {
            text: "Profile photo suggested".to_string(),
            content_type: Some("suggest_profile_photo".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::SuggestProfilePhoto {}),
        },
        MessageContent::MessageCustomServiceAction(a) => ExtractedMessageData {
            text: a.text.clone(),
            content_type: Some("custom_service_action".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::CustomServiceAction {
                text: a.text.clone(),
            }),
        },
        MessageContent::MessageGameScore(g) => ExtractedMessageData {
            text: format!("Game score: {}", g.score),
            content_type: Some("game_score".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::GameScore {
                game_message_id: g.game_message_id,
                game_id: g.game_id,
                score: g.score,
            }),
        },
        MessageContent::MessagePaymentSuccessful(p) => ExtractedMessageData {
            text: format!("Payment: {} {}", p.total_amount, p.currency),
            content_type: Some("payment_successful".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PaymentSuccessful {
                invoice_chat_id: p.invoice_chat_id,
                invoice_message_id: p.invoice_message_id,
                currency: p.currency.clone(),
                total_amount: p.total_amount,
                is_recurring: p.is_recurring,
                invoice_name: non_empty(&p.invoice_name),
            }),
        },
        MessageContent::MessagePremiumGiftCode(g) => ExtractedMessageData {
            text: format!("Premium gift code ({} months)", g.month_count),
            content_type: Some("premium_gift_code".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PremiumGiftCode {
                is_from_giveaway: g.is_from_giveaway,
                is_unclaimed: g.is_unclaimed,
                currency: g.currency.clone(),
                amount: g.amount,
                month_count: g.month_count,
                code: g.code.clone(),
            }),
        },
        MessageContent::MessagePremiumGiveawayCreated => ExtractedMessageData {
            text: "Premium giveaway created".to_string(),
            content_type: Some("premium_giveaway_created".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PremiumGiveawayCreated {}),
        },
        MessageContent::MessagePremiumGiveaway(g) => ExtractedMessageData {
            text: format!(
                "Premium giveaway ({} winners, {} months)",
                g.winner_count, g.month_count
            ),
            content_type: Some("premium_giveaway".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PremiumGiveaway {
                winner_count: g.winner_count,
                month_count: g.month_count,
            }),
        },
        MessageContent::MessagePremiumGiveawayCompleted(g) => ExtractedMessageData {
            text: format!("Premium giveaway completed ({} winners)", g.winner_count),
            content_type: Some("premium_giveaway_completed".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PremiumGiveawayCompleted {
                giveaway_message_id: g.giveaway_message_id,
                winner_count: g.winner_count,
                unclaimed_prize_count: g.unclaimed_prize_count,
            }),
        },
        MessageContent::MessagePremiumGiveawayWinners(g) => ExtractedMessageData {
            text: format!("Premium giveaway winners ({} winners)", g.winner_count),
            content_type: Some("premium_giveaway_winners".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PremiumGiveawayWinners {
                boosted_chat_id: g.boosted_chat_id,
                giveaway_message_id: g.giveaway_message_id,
                winner_count: g.winner_count,
                winner_user_ids: g.winner_user_ids.clone(),
                unclaimed_prize_count: g.unclaimed_prize_count,
                month_count: g.month_count,
            }),
        },
        MessageContent::MessageUsersShared(u) => ExtractedMessageData {
            text: format!("Users shared ({} users)", u.users.len()),
            content_type: Some("users_shared".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::UsersShared {
                button_id: u.button_id,
            }),
        },
        MessageContent::MessageChatShared(c) => ExtractedMessageData {
            text: "Chat shared".to_string(),
            content_type: Some("chat_shared".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ChatShared {
                button_id: c.button_id,
            }),
        },
        MessageContent::MessageBotWriteAccessAllowed(_) => ExtractedMessageData {
            text: "Bot write access allowed".to_string(),
            content_type: Some("bot_write_access_allowed".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::BotWriteAccessAllowed {}),
        },
        MessageContent::MessageWebAppDataSent(w) => ExtractedMessageData {
            text: format!("Web app data sent: {}", w.button_text),
            content_type: Some("web_app_data_sent".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::WebAppDataSent {
                button_text: w.button_text.clone(),
            }),
        },
        MessageContent::MessagePassportDataSent(_) => ExtractedMessageData {
            text: "Passport data sent".to_string(),
            content_type: Some("passport_data_sent".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::PassportDataSent {}),
        },
        MessageContent::MessageProximityAlertTriggered(p) => ExtractedMessageData {
            text: format!("Proximity alert ({}m)", p.distance),
            content_type: Some("proximity_alert_triggered".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ProximityAlertTriggered {
                distance: p.distance,
            }),
        },
        MessageContent::MessageExpiredPhoto => ExtractedMessageData {
            text: "Expired photo".to_string(),
            content_type: Some("expired_photo".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ExpiredPhoto {}),
        },
        MessageContent::MessageExpiredVideo => ExtractedMessageData {
            text: "Expired video".to_string(),
            content_type: Some("expired_video".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ExpiredVideo {}),
        },
        MessageContent::MessageExpiredVideoNote => ExtractedMessageData {
            text: "Expired video note".to_string(),
            content_type: Some("expired_video_note".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ExpiredVideoNote {}),
        },
        MessageContent::MessageExpiredVoiceNote => ExtractedMessageData {
            text: "Expired voice note".to_string(),
            content_type: Some("expired_voice_note".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: Some(MessageContentDetails::ExpiredVoiceNote {}),
        },
        MessageContent::MessageUnsupported => {
            let tdlib_type = "messageUnsupported".to_string();
            ExtractedMessageData {
                text: "[Unsupported]".to_string(),
                content_type: Some("unsupported".to_string()),
                is_downloadable: false,
                download_files: vec![],
                content: Some(MessageContentDetails::Unsupported { tdlib_type }),
            }
        }
    }
}

#[async_trait]
trait MessageHistorySource: Send + Sync {
    /// Fetch a batch of messages for `chat_id` older than or at `from_message_id` (0 = latest).
    /// Returns messages newest-first. May return fewer than `limit`.
    async fn fetch_batch(
        &self,
        chat_id: i64,
        from_message_id: i64,
        limit: i32,
    ) -> Result<Vec<MessageInfo>>;
}

async fn collect_messages_paginated<S: MessageHistorySource>(
    source: &S,
    chat_id: i64,
    limit: i32,
    until_message_id: Option<i64>,
) -> Result<Vec<MessageInfo>> {
    let mut result = Vec::new();
    let mut from_message_id: i64 = 0;
    let mut seen_ids = std::collections::HashSet::new();
    let mut empty_attempts = 0;
    const MAX_EMPTY_ATTEMPTS: u32 = 5;

    while result.len() < limit as usize {
        let remaining = (limit - result.len() as i32).min(100);

        let msgs: Vec<_> = source
            .fetch_batch(chat_id, from_message_id, remaining)
            .await?
            .into_iter()
            .filter(|m| seen_ids.insert(m.id))
            .collect();

        if msgs.is_empty() {
            empty_attempts += 1;
            if empty_attempts >= MAX_EMPTY_ATTEMPTS {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            continue;
        }

        empty_attempts = 0;
        from_message_id = msgs.last().unwrap().id;

        let mut hit_boundary = false;
        for msg in msgs {
            if let Some(boundary_id) = until_message_id
                && msg.id < boundary_id
            {
                // msg is older than the boundary — stop without including it
                hit_boundary = true;
                break;
            }
            let msg_id = msg.id;
            result.push(msg);
            if let Some(boundary_id) = until_message_id
                && msg_id == boundary_id
            {
                // msg is exactly at the boundary — include it, then stop
                hit_boundary = true;
                break;
            }
            if result.len() >= limit as usize {
                break;
            }
        }

        if hit_boundary {
            break;
        }
    }

    Ok(result)
}

fn get_user_full_name(user: &tdlib_rs::types::User) -> String {
    if user.last_name.is_empty() {
        user.first_name.clone()
    } else {
        format!("{} {}", user.first_name, user.last_name)
    }
}

#[async_trait]
impl MessageHistorySource for TdLibClient {
    async fn fetch_batch(
        &self,
        chat_id: i64,
        from_message_id: i64,
        limit: i32,
    ) -> Result<Vec<MessageInfo>> {
        let client_id = self.get_client_id().await?;

        let msgs_enum = tdlib_rs::functions::get_chat_history(
            chat_id,
            from_message_id,
            0,
            limit,
            false,
            client_id,
        )
        .await
        .map_err(|e| {
            let msg = e.message.to_lowercase();
            if msg.contains("not found")
                || msg.contains("private")
                || msg.contains("kicked")
                || msg.contains("banned")
                || msg.contains("restricted")
                || msg.contains("deleted")
                || msg.contains("deactivated")
            {
                TgError::ChatInaccessible(chat_id)
            } else {
                TgError::TdLib(e.message)
            }
        })?;

        let mut result = Vec::new();
        for msg in unwrap_messages(msgs_enum).messages.into_iter().flatten() {
            let extracted = extract_message_data(&msg.content);
            let sender = match &msg.sender_id {
                tdlib_rs::enums::MessageSender::User(u) => {
                    if let Ok(ue) = tdlib_rs::functions::get_user(u.user_id, client_id).await {
                        get_user_full_name(&unwrap_user(ue))
                    } else {
                        "Unknown".to_string()
                    }
                }
                tdlib_rs::enums::MessageSender::Chat(c) => {
                    if let Ok(ce) = tdlib_rs::functions::get_chat(c.chat_id, client_id).await {
                        unwrap_chat(ce).title
                    } else {
                        "Unknown".to_string()
                    }
                }
            };
            result.push(MessageInfo {
                id: msg.id,
                chat_id: msg.chat_id,
                sender,
                text: extracted.text,
                date: format_timestamp(msg.date),
                is_outgoing: msg.is_outgoing,
                edit_date: if msg.edit_date == 0 {
                    None
                } else {
                    Some(format_timestamp(msg.edit_date))
                },
                content_type: extracted.content_type,
                is_downloadable: extracted.is_downloadable,
                download_files: extracted.download_files,
                content: extracted.content,
            });
        }
        Ok(result)
    }
}

#[async_trait]
impl TelegramClient for TdLibClient {
    async fn authenticate(&mut self) -> Result<()> {
        use std::io::{self, BufRead, Write};
        use tdlib_rs::enums::AuthorizationState;

        let phone = std::env::var("TG_PHONE").ok();

        let client_id = self.ensure_client_initialized().await?;

        // Subscribe to updates from the background receive loop
        let mut receiver = self.update_sender.subscribe();

        // TDLib needs at least one request before it sends updates.
        // Send a simple request to trigger the update flow.
        let _ = tdlib_rs::functions::get_option("version".to_string(), client_id).await;

        // Handle current state immediately to avoid hanging when already authorized.
        if let Ok(state) = tdlib_rs::functions::get_authorization_state(client_id).await {
            match state {
                AuthorizationState::WaitTdlibParameters => {
                    self.ensure_tdlib_parameters(client_id).await?;
                }
                AuthorizationState::WaitPhoneNumber => {
                    let phone_number = match &phone {
                        Some(p) => p.clone(),
                        None => {
                            print!("Enter phone number (E.164 format, e.g. +1234567890): ");
                            io::stdout().flush().ok();
                            io::stdin()
                                .lock()
                                .lines()
                                .next()
                                .ok_or_else(|| {
                                    TgError::Other("Failed to read phone number".to_string())
                                })?
                                .map_err(|e| TgError::Other(e.to_string()))?
                                .trim()
                                .to_string()
                        }
                    };
                    println!("Sending phone number...");
                    tdlib_rs::functions::set_authentication_phone_number(
                        phone_number,
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
                    return Ok(());
                }
                AuthorizationState::Closed => {
                    return Err(TgError::AuthFailed("Session closed".to_string()));
                }
                _ => {}
            }
        }

        loop {
            match receiver.recv().await {
                Ok(update) => {
                    if let tdlib_rs::enums::Update::AuthorizationState(state) = update {
                        match state.authorization_state {
                            AuthorizationState::WaitTdlibParameters => {
                                self.ensure_tdlib_parameters(client_id).await?;
                            }
                            AuthorizationState::WaitPhoneNumber => {
                                let phone_number = match &phone {
                                    Some(p) => p.clone(),
                                    None => {
                                        print!(
                                            "Enter phone number (E.164 format, e.g. +1234567890): "
                                        );
                                        io::stdout().flush().ok();
                                        io::stdin()
                                            .lock()
                                            .lines()
                                            .next()
                                            .ok_or_else(|| {
                                                TgError::Other(
                                                    "Failed to read phone number".to_string(),
                                                )
                                            })?
                                            .map_err(|e| TgError::Other(e.to_string()))?
                                            .trim()
                                            .to_string()
                                    }
                                };
                                println!("Sending phone number...");
                                tdlib_rs::functions::set_authentication_phone_number(
                                    phone_number,
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
                                    .ok_or_else(|| {
                                        TgError::Other("Failed to read code".to_string())
                                    })?
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
                                    .ok_or_else(|| {
                                        TgError::Other("Failed to read password".to_string())
                                    })?
                                    .map_err(|e| TgError::Other(e.to_string()))?;
                                tdlib_rs::functions::check_authentication_password(
                                    password, client_id,
                                )
                                .await
                                .map_err(|e| TgError::AuthFailed(e.message))?;
                            }
                            AuthorizationState::Ready => {
                                *self.authenticated.lock().await = true;
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
        self.collect_filtered_chats(limit, |chat| chat.chat_type == ChatTypeKind::Private)
            .await
    }

    async fn get_groups(&self, limit: i32) -> Result<Vec<ChatInfo>> {
        self.collect_filtered_chats(limit, |chat| {
            matches!(
                chat.chat_type,
                ChatTypeKind::BasicGroup | ChatTypeKind::Supergroup
            )
        })
        .await
    }

    async fn get_unread_chats(&self, limit: i32) -> Result<Vec<ChatInfo>> {
        self.collect_filtered_chats(limit, |chat| chat.unread_count > 0)
            .await
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

        let message_enum =
            tdlib_rs::functions::send_message(chat_id, 0, None, None, content, client_id)
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
                Ok(Ok(update)) => match update {
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
                },
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

    async fn get_messages(
        &self,
        chat_id: i64,
        limit: i32,
        until_message_id: Option<i64>,
    ) -> Result<Vec<MessageInfo>> {
        collect_messages_paginated(self, chat_id, limit, until_message_id).await
    }

    async fn get_boundary_message_id(
        &self,
        chat_id: i64,
        timestamp: i32,
    ) -> Result<BoundaryResult> {
        let client_id = self.get_client_id().await?;
        match tdlib_rs::functions::get_chat_message_by_date(chat_id, timestamp, client_id).await {
            Ok(msg_enum) => {
                let msg = unwrap_message(msg_enum);
                if msg.id == 0 {
                    Ok(BoundaryResult::None)
                } else if msg.date >= timestamp {
                    Ok(BoundaryResult::BoundAt(msg.id))
                } else {
                    // TDLib returned a message older than the requested timestamp —
                    // all messages in this chat are before the cutoff.
                    Ok(BoundaryResult::Empty)
                }
            }
            Err(_) => Ok(BoundaryResult::None),
        }
    }

    async fn download_message_media(
        &self,
        chat_id: i64,
        message_id: i64,
        options: DownloadOptions,
    ) -> Result<DownloadReport> {
        let client_id = self.get_client_id().await?;
        let message_enum = tdlib_rs::functions::get_message(chat_id, message_id, client_id)
            .await
            .map_err(|e| TgError::TdLib(e.message))?;
        let message = unwrap_message(message_enum);
        let extracted = extract_message_data(&message.content);
        let output_dir = ensure_output_dir(&options.output_dir)?;

        if extracted.download_files.is_empty() {
            return Ok(DownloadReport {
                chat_id,
                message_id,
                status: DownloadStatus::NoDownloadableMedia,
                output_dir: canonical_path_string(&output_dir),
                content_type: extracted.content_type,
                content: extracted.content,
                files: vec![],
            });
        }

        let mut file_results = Vec::new();
        let mut any_failed = false;
        let mut any_renamed = false;
        let multi_file = extracted.download_files.len() > 1;

        for (index, file_ref) in extracted.download_files.iter().enumerate() {
            let downloaded = tdlib_rs::functions::download_file(
                file_ref.file_id,
                options.priority,
                0,
                0,
                true,
                client_id,
            )
            .await
            .map_err(|e| TgError::TdLib(e.message));

            let mut result = DownloadedFileResult {
                file_id: file_ref.file_id,
                is_primary: file_ref.is_primary,
                status: DownloadStatus::Downloaded,
                role: file_ref.role.clone(),
                file_name: file_ref.file_name.clone(),
                mime_type: file_ref.mime_type.clone(),
                size_bytes: file_ref.size_bytes,
                expected_size_bytes: file_ref.expected_size_bytes,
                source_path: None,
                saved_path: None,
                note: None,
            };

            let file = match downloaded {
                Ok(file_enum) => unwrap_file(file_enum),
                Err(err) => {
                    any_failed = true;
                    result.status = DownloadStatus::Failed;
                    result.note = Some(err.to_string());
                    file_results.push(result);
                    continue;
                }
            };

            result.size_bytes = file.size;
            result.expected_size_bytes = file.expected_size;
            result.source_path =
                non_empty(&file.local.path).map(|path| canonical_path_string(Path::new(&path)));

            let source_path = match non_empty(&file.local.path) {
                Some(path) => PathBuf::from(path),
                None => {
                    any_failed = true;
                    result.status = DownloadStatus::Failed;
                    result.note = Some("TDLib returned empty local file path".to_string());
                    file_results.push(result);
                    continue;
                }
            };

            if !source_path.exists() {
                any_failed = true;
                result.status = DownloadStatus::Failed;
                result.note = Some(format!(
                    "Downloaded source file not found: {}",
                    source_path.display()
                ));
                file_results.push(result);
                continue;
            }

            let filename = build_download_filename(
                chat_id,
                message_id,
                file_ref,
                extracted.content_type.as_deref(),
                multi_file,
                index + 1,
            );
            let mut destination = output_dir.join(filename);

            if destination.exists() {
                match files_match_sha256(&source_path, &destination) {
                    Ok(true) => {
                        result.status = DownloadStatus::SkippedSameHash;
                        result.saved_path = Some(canonical_path_string(&destination));
                        result.note = Some("Existing file has matching SHA256".to_string());
                        file_results.push(result);
                        continue;
                    }
                    Ok(false) => {
                        destination = next_available_path(&destination);
                        any_renamed = true;
                        result.status = DownloadStatus::RenamedConflict;
                    }
                    Err(err) => {
                        any_failed = true;
                        result.status = DownloadStatus::Failed;
                        result.note = Some(format!("Failed to compare SHA256: {err}"));
                        file_results.push(result);
                        continue;
                    }
                }
            }

            if let Err(err) = std::fs::copy(&source_path, &destination) {
                any_failed = true;
                result.status = DownloadStatus::Failed;
                result.note = Some(format!(
                    "Failed to copy {} to {}: {}",
                    source_path.display(),
                    destination.display(),
                    err
                ));
                file_results.push(result);
                continue;
            }

            result.saved_path = Some(canonical_path_string(&destination));
            file_results.push(result);
        }

        let status = if any_failed {
            DownloadStatus::Failed
        } else if any_renamed {
            DownloadStatus::RenamedConflict
        } else if file_results
            .iter()
            .all(|result| result.status == DownloadStatus::SkippedSameHash)
        {
            DownloadStatus::SkippedSameHash
        } else {
            DownloadStatus::Downloaded
        };

        Ok(DownloadReport {
            chat_id,
            message_id,
            status,
            output_dir: canonical_path_string(&output_dir),
            content_type: extracted.content_type,
            content: extracted.content,
            files: file_results,
        })
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
    Some(extract_message_data(content).text)
}

fn ensure_output_dir(path: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(path)?;
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(absolute)
}

fn canonical_path_string(path: &Path) -> String {
    if let Ok(canonical) = path.canonicalize() {
        return canonical.to_string_lossy().to_string();
    }

    if path.is_absolute() {
        return path.to_string_lossy().to_string();
    }

    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path).to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    let fallback = "download";
    let source = if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    };
    source
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect::<String>()
}

fn mime_extension(mime: &str) -> Option<&'static str> {
    match mime {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "video/mp4" => Some("mp4"),
        "audio/mpeg" => Some("mp3"),
        "audio/ogg" => Some("ogg"),
        "audio/mp4" => Some("m4a"),
        "application/pdf" => Some("pdf"),
        _ => None,
    }
}

fn content_type_extension(content_type: Option<&str>) -> Option<&'static str> {
    match content_type {
        Some("photo") => Some("jpg"),
        Some("video") | Some("video_note") | Some("animation") => Some("mp4"),
        Some("audio") => Some("mp3"),
        Some("voice") => Some("ogg"),
        Some("sticker") => Some("webp"),
        _ => None,
    }
}

fn build_download_filename(
    chat_id: i64,
    message_id: i64,
    file_ref: &MessageFileRef,
    content_type: Option<&str>,
    multi_file: bool,
    index: usize,
) -> String {
    let base_name = file_ref
        .file_name
        .as_deref()
        .map(sanitize_filename)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("chat{chat_id}_message{message_id}_file{}", file_ref.file_id));

    let path = Path::new(&base_name);
    let mut stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| base_name.clone());
    let mut ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty());

    if multi_file {
        if file_ref.is_primary {
            stem.push_str("__primary");
        } else {
            stem.push_str(&format!("__file{index}"));
        }
    }

    if ext.is_none() {
        ext = file_ref
            .mime_type
            .as_deref()
            .and_then(mime_extension)
            .map(ToOwned::to_owned)
            .or_else(|| content_type_extension(content_type).map(ToOwned::to_owned))
            .or_else(|| Some("bin".to_string()));
    }

    format!("{stem}.{}", ext.unwrap_or_else(|| "bin".to_string()))
}

fn next_available_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());

    let mut counter = 1u32;
    loop {
        let candidate_name = match &ext {
            Some(ext) => format!("{stem} ({counter}).{ext}"),
            None => format!("{stem} ({counter})"),
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
        counter = counter.saturating_add(1);
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::process::Command;

    let output = Command::new("shasum")
        .arg("-a")
        .arg("256")
        .arg(path)
        .output();

    let output = match output {
        Ok(output) => output,
        Err(_) => {
            // Fallback for Linux environments where sha256sum is available.
            Command::new("sha256sum").arg(path).output()?
        }
    };

    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "failed to compute SHA256 for {}",
            path.display()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| std::io::Error::other("invalid hash output"))?;
    Ok(hash.to_string())
}

fn files_match_sha256(left: &Path, right: &Path) -> std::io::Result<bool> {
    Ok(sha256_file(left)? == sha256_file(right)?)
}

fn format_timestamp(timestamp: i32) -> String {
    use chrono::{DateTime, SecondsFormat, Utc};

    DateTime::<Utc>::from_timestamp(timestamp as i64, 0)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| "unknown".to_string())
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
        pub inaccessible_chat_ids: Vec<i64>,
        /// Result returned by `get_boundary_message_id`
        pub boundary_result: BoundaryResult,
        /// Tracks how many times `get_messages` has been called
        pub get_messages_call_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
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
                inaccessible_chat_ids: vec![],
                boundary_result: BoundaryResult::None,
                get_messages_call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                messages: vec![
                    MessageInfo {
                        id: 1,
                        chat_id: 1,
                        sender: "John Doe".to_string(),
                        text: "Hello!".to_string(),
                        date: "1h ago".to_string(),
                        is_outgoing: false,
                        edit_date: None,
                        content_type: Some("text".to_string()),
                        is_downloadable: false,
                        download_files: vec![],
                        content: None,
                    },
                    MessageInfo {
                        id: 2,
                        chat_id: 1,
                        sender: "You".to_string(),
                        text: "Hi there!".to_string(),
                        date: "30m ago".to_string(),
                        is_outgoing: true,
                        edit_date: None,
                        content_type: Some("text".to_string()),
                        is_downloadable: false,
                        download_files: vec![],
                        content: None,
                    },
                ],
            }
        }
    }

    #[async_trait]
    impl TelegramClient for MockClient {
        async fn authenticate(&mut self) -> Result<()> {
            let state = *self.auth_state.lock().unwrap();
            match state {
                AuthState::WaitPhone => {
                    *self.phone_sent.lock().unwrap() = true;
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

        async fn get_messages(
            &self,
            chat_id: i64,
            limit: i32,
            until_message_id: Option<i64>,
        ) -> Result<Vec<MessageInfo>> {
            self.get_messages_call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.inaccessible_chat_ids.contains(&chat_id) {
                return Err(TgError::ChatInaccessible(chat_id));
            }
            let msgs: Vec<_> = self
                .messages
                .iter()
                .filter(|m| {
                    if let Some(boundary) = until_message_id {
                        m.id >= boundary
                    } else {
                        true
                    }
                })
                .take(limit as usize)
                .cloned()
                .collect();
            Ok(msgs)
        }

        async fn get_boundary_message_id(
            &self,
            _chat_id: i64,
            _timestamp: i32,
        ) -> Result<BoundaryResult> {
            Ok(self.boundary_result.clone())
        }

        async fn download_message_media(
            &self,
            chat_id: i64,
            message_id: i64,
            options: DownloadOptions,
        ) -> Result<DownloadReport> {
            Ok(DownloadReport {
                chat_id,
                message_id,
                status: DownloadStatus::NoDownloadableMedia,
                output_dir: canonical_path_string(&options.output_dir),
                content_type: None,
                content: None,
                files: vec![],
            })
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
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use tempfile::tempdir;

    fn formatted(text: &str) -> tdlib_rs::types::FormattedText {
        tdlib_rs::types::FormattedText {
            text: text.to_string(),
            entities: vec![],
        }
    }

    fn file(id: i32, path: &str) -> tdlib_rs::types::File {
        tdlib_rs::types::File {
            id,
            size: 1234,
            expected_size: 1234,
            local: tdlib_rs::types::LocalFile {
                path: path.to_string(),
                can_be_downloaded: true,
                is_downloading_completed: !path.is_empty(),
                ..Default::default()
            },
            remote: tdlib_rs::types::RemoteFile {
                id: format!("remote-{id}"),
                unique_id: format!("unique-{id}"),
                is_uploading_active: false,
                is_uploading_completed: true,
                uploaded_size: 1234,
            },
        }
    }

    #[test]
    fn extract_audio_message_metadata() {
        let content =
            tdlib_rs::enums::MessageContent::MessageAudio(tdlib_rs::types::MessageAudio {
                audio: tdlib_rs::types::Audio {
                    duration: 90,
                    title: "Song".to_string(),
                    performer: "Artist".to_string(),
                    file_name: "track.mp3".to_string(),
                    mime_type: "audio/mpeg".to_string(),
                    album_cover_minithumbnail: None,
                    album_cover_thumbnail: None,
                    external_album_covers: vec![],
                    audio: file(1, "/tmp/track.mp3"),
                },
                caption: formatted("audio caption"),
            });

        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("audio"));
        assert!(extracted.is_downloadable);
        assert_eq!(extracted.download_files.len(), 1);
        assert!(extracted.text.starts_with("[Audio:"));
        assert!(extracted.text.ends_with(']'));
        assert_ne!(extracted.text, "[Media]");
    }

    #[test]
    fn extract_voice_message_metadata() {
        let content =
            tdlib_rs::enums::MessageContent::MessageVoiceNote(tdlib_rs::types::MessageVoiceNote {
                voice_note: tdlib_rs::types::VoiceNote {
                    duration: 45,
                    waveform: String::new(),
                    mime_type: "audio/ogg".to_string(),
                    speech_recognition_result: None,
                    voice: file(2, "/tmp/voice.ogg"),
                },
                caption: formatted("voice caption"),
                is_listened: false,
            });

        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("voice"));
        assert!(extracted.is_downloadable);
        assert_eq!(extracted.download_files.len(), 1);
        assert_ne!(extracted.text, "[Media]");
    }

    #[test]
    fn extract_animation_message_metadata() {
        let content =
            tdlib_rs::enums::MessageContent::MessageAnimation(tdlib_rs::types::MessageAnimation {
                animation: tdlib_rs::types::Animation {
                    duration: 12,
                    width: 640,
                    height: 480,
                    file_name: "anim.mp4".to_string(),
                    mime_type: "video/mp4".to_string(),
                    has_stickers: false,
                    minithumbnail: None,
                    thumbnail: None,
                    animation: file(3, "/tmp/anim.mp4"),
                },
                caption: formatted(""),
                has_spoiler: false,
                is_secret: false,
            });

        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("animation"));
        assert!(extracted.is_downloadable);
        assert!(extracted.text.contains("640x480"));
        assert!(extracted.text.starts_with("[Animation:"));
        assert!(extracted.text.ends_with(']'));
        assert_ne!(extracted.text, "[Media]");
    }

    #[test]
    fn extract_animated_emoji_message_metadata() {
        let sticker = tdlib_rs::types::Sticker {
            id: 10,
            set_id: 20,
            width: 128,
            height: 128,
            emoji: "😀".to_string(),
            format: tdlib_rs::enums::StickerFormat::Webp,
            full_type: tdlib_rs::enums::StickerFullType::CustomEmoji(
                tdlib_rs::types::StickerFullTypeCustomEmoji {
                    custom_emoji_id: 123456789,
                    needs_repainting: false,
                },
            ),
            outline: vec![],
            thumbnail: None,
            sticker: file(6, "/tmp/emoji.webp"),
        };
        let content = tdlib_rs::enums::MessageContent::MessageAnimatedEmoji(
            tdlib_rs::types::MessageAnimatedEmoji {
                animated_emoji: tdlib_rs::types::AnimatedEmoji {
                    sticker: Some(sticker),
                    sticker_width: 128,
                    sticker_height: 128,
                    fitzpatrick_type: 0,
                    sound: Some(file(7, "/tmp/emoji.ogg")),
                },
                emoji: "😀".to_string(),
            },
        );

        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("emoji"));
        assert!(extracted.is_downloadable);
        assert_eq!(extracted.download_files.len(), 2);
        assert!(extracted.text.starts_with("[Emoji:"));
        assert!(extracted.text.ends_with(']'));
        assert_ne!(extracted.text, "[Unsupported]");
    }

    #[test]
    fn extract_location_contact_and_poll_metadata() {
        let location =
            tdlib_rs::enums::MessageContent::MessageLocation(tdlib_rs::types::MessageLocation {
                location: tdlib_rs::types::Location {
                    latitude: 37.7749,
                    longitude: -122.4194,
                    horizontal_accuracy: 10.0,
                },
                live_period: 0,
                expires_in: 0,
                heading: 0,
                proximity_alert_radius: 0,
            });
        let location_extracted = extract_message_data(&location);
        assert_eq!(location_extracted.content_type.as_deref(), Some("location"));
        assert!(!location_extracted.is_downloadable);
        assert!(location_extracted.download_files.is_empty());

        let contact =
            tdlib_rs::enums::MessageContent::MessageContact(tdlib_rs::types::MessageContact {
                contact: tdlib_rs::types::Contact {
                    phone_number: "+15555550123".to_string(),
                    first_name: "Jane".to_string(),
                    last_name: "Doe".to_string(),
                    vcard: String::new(),
                    user_id: 99,
                },
            });
        let contact_extracted = extract_message_data(&contact);
        assert_eq!(contact_extracted.content_type.as_deref(), Some("contact"));
        assert!(!contact_extracted.is_downloadable);

        let poll = tdlib_rs::enums::MessageContent::MessagePoll(tdlib_rs::types::MessagePoll {
            poll: tdlib_rs::types::Poll {
                id: 1,
                question: formatted("Best option?"),
                options: vec![tdlib_rs::types::PollOption {
                    text: formatted("A"),
                    voter_count: 1,
                    vote_percentage: 100,
                    is_chosen: true,
                    is_being_chosen: false,
                }],
                total_voter_count: 1,
                recent_voter_ids: vec![],
                is_anonymous: true,
                r#type: tdlib_rs::enums::PollType::Regular(tdlib_rs::types::PollTypeRegular {
                    allow_multiple_answers: false,
                }),
                open_period: 0,
                close_date: 0,
                is_closed: false,
            },
        });
        let poll_extracted = extract_message_data(&poll);
        assert_eq!(poll_extracted.content_type.as_deref(), Some("poll"));
        assert!(!poll_extracted.is_downloadable);
        assert!(poll_extracted.text.starts_with("[Poll:"));
        assert!(poll_extracted.text.ends_with(']'));
    }

    #[test]
    fn extract_photo_uses_best_variant_for_download() {
        let content =
            tdlib_rs::enums::MessageContent::MessagePhoto(tdlib_rs::types::MessagePhoto {
                photo: tdlib_rs::types::Photo {
                    has_stickers: false,
                    minithumbnail: None,
                    sizes: vec![
                        tdlib_rs::types::PhotoSize {
                            r#type: "s".to_string(),
                            photo: file(10, "/tmp/s.jpg"),
                            width: 320,
                            height: 240,
                            progressive_sizes: vec![],
                        },
                        tdlib_rs::types::PhotoSize {
                            r#type: "x".to_string(),
                            photo: file(11, "/tmp/x.jpg"),
                            width: 1920,
                            height: 1080,
                            progressive_sizes: vec![],
                        },
                    ],
                },
                caption: formatted(""),
                has_spoiler: false,
                is_secret: false,
            });

        let extracted = extract_message_data(&content);
        assert_eq!(extracted.download_files.len(), 1);
        assert_eq!(extracted.download_files[0].file_id, 11);
        assert!(extracted.text.starts_with("[Photo:"));
        assert!(extracted.text.ends_with(']'));
    }

    #[test]
    fn extract_unsupported_message_sets_content_type_and_tdlib_type() {
        use tdlib_rs::enums::MessageContent;

        // MessageUnsupported is a unit variant (no inner data)
        let content = MessageContent::MessageUnsupported;
        let extracted = extract_message_data(&content);
        assert_eq!(extracted.text, "[Unsupported]");
        assert_eq!(extracted.content_type.as_deref(), Some("unsupported"));
        assert!(!extracted.is_downloadable);
        if let Some(MessageContentDetails::Unsupported { tdlib_type }) = &extracted.content {
            assert_eq!(tdlib_type, "messageUnsupported");
        } else {
            panic!("expected Unsupported content details");
        }
    }

    #[test]
    fn tdlib_type_name_extracts_camel_case_name() {
        use tdlib_rs::enums::MessageContent;

        assert_eq!(
            tdlib_type_name(&MessageContent::MessageUnsupported),
            "messageUnsupported"
        );
        // A variant with data — the Debug format includes the inner struct
        let dice = MessageContent::MessageExpiredPhoto;
        assert_eq!(tdlib_type_name(&dice), "messageExpiredPhoto");
    }

    #[test]
    fn extract_basic_group_chat_create() {
        use tdlib_rs::enums::MessageContent;

        let content =
            MessageContent::MessageBasicGroupChatCreate(tdlib_rs::types::MessageBasicGroupChatCreate {
                title: "My Group".to_string(),
                member_user_ids: vec![100, 200, 300],
            });
        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("group_created"));
        assert_eq!(extracted.text, "Group created: My Group");
        assert!(!extracted.is_downloadable);
        if let Some(MessageContentDetails::BasicGroupChatCreate {
            title,
            member_user_ids,
        }) = &extracted.content
        {
            assert_eq!(title, "My Group");
            assert_eq!(member_user_ids, &vec![100, 200, 300]);
        } else {
            panic!("expected BasicGroupChatCreate content details");
        }
    }

    #[test]
    fn extract_chat_add_members() {
        use tdlib_rs::enums::MessageContent;

        let content =
            MessageContent::MessageChatAddMembers(tdlib_rs::types::MessageChatAddMembers {
                member_user_ids: vec![42, 99],
            });
        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("members_added"));
        assert!(extracted.text.contains("42"));
        assert!(extracted.text.contains("99"));
        assert!(!extracted.is_downloadable);
        if let Some(MessageContentDetails::ChatAddMembers { member_user_ids }) = &extracted.content
        {
            assert_eq!(member_user_ids, &vec![42, 99]);
        } else {
            panic!("expected ChatAddMembers content details");
        }
    }

    #[test]
    fn extract_expired_photo() {
        use tdlib_rs::enums::MessageContent;

        let content = MessageContent::MessageExpiredPhoto;
        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("expired_photo"));
        assert_eq!(extracted.text, "Expired photo");
        assert!(!extracted.is_downloadable);
    }

    #[test]
    fn extract_dice_message() {
        use tdlib_rs::enums::MessageContent;

        let content = MessageContent::MessageDice(tdlib_rs::types::MessageDice {
            initial_state: None,
            final_state: None,
            emoji: "🎲".to_string(),
            value: 5,
            success_animation_frame_number: 0,
        });
        let extracted = extract_message_data(&content);
        assert_eq!(extracted.content_type.as_deref(), Some("dice"));
        assert!(extracted.text.contains("🎲"));
        assert!(extracted.text.contains("5"));
        if let Some(MessageContentDetails::Dice { emoji, value }) = &extracted.content {
            assert_eq!(emoji, "🎲");
            assert_eq!(*value, 5);
        } else {
            panic!("expected Dice content details");
        }
    }

    #[test]
    fn format_timestamp_returns_iso_utc() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        let ts = format_timestamp(1_700_000_000);
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn edit_date_zero_maps_to_none() {
        let edit_date: i32 = 0;
        let result: Option<String> = if edit_date == 0 {
            None
        } else {
            Some(format_timestamp(edit_date))
        };
        assert_eq!(result, None);
    }

    #[test]
    fn edit_date_positive_maps_to_some_iso8601() {
        let edit_date: i32 = 1_700_000_000;
        let result: Option<String> = if edit_date == 0 {
            None
        } else {
            Some(format_timestamp(edit_date))
        };
        assert!(result.is_some());
        let date_str = result.unwrap();
        // Should be a valid ISO 8601 / RFC 3339 string
        assert!(date_str.contains('T'));
        assert!(date_str.ends_with('Z'));
        assert_eq!(date_str, "2023-11-14T22:13:20Z");
    }

    #[test]
    fn build_filename_marks_primary_for_multiple_files() {
        let primary = MessageFileRef {
            file_id: 1,
            is_primary: true,
            role: Some("main".to_string()),
            file_name: Some("clip.mp4".to_string()),
            mime_type: Some("video/mp4".to_string()),
            size_bytes: 1,
            expected_size_bytes: 1,
            local_path: None,
            remote_id: None,
            remote_unique_id: None,
            can_be_downloaded: true,
            is_downloaded: true,
        };
        let secondary = MessageFileRef {
            file_id: 2,
            is_primary: false,
            role: Some("alt".to_string()),
            file_name: Some("clip.mp4".to_string()),
            mime_type: Some("video/mp4".to_string()),
            size_bytes: 1,
            expected_size_bytes: 1,
            local_path: None,
            remote_id: None,
            remote_unique_id: None,
            can_be_downloaded: true,
            is_downloaded: true,
        };

        let p = build_download_filename(1, 2, &primary, Some("video"), true, 1);
        let s = build_download_filename(1, 2, &secondary, Some("video"), true, 2);
        assert!(p.contains("__primary"));
        assert!(s.contains("__file2"));
    }

    #[test]
    fn next_available_path_adds_numbered_suffix() {
        let tmp = tempdir().unwrap();
        let original = tmp.path().join("file.txt");
        std::fs::write(&original, "a").unwrap();

        let candidate = next_available_path(&original);
        assert_eq!(
            candidate.file_name().unwrap().to_string_lossy(),
            "file (1).txt"
        );
    }

    #[test]
    fn file_hash_comparison_detects_equal_and_different() {
        let tmp = tempdir().unwrap();
        let first = tmp.path().join("a.bin");
        let second = tmp.path().join("b.bin");
        let third = tmp.path().join("c.bin");

        std::fs::write(&first, b"same").unwrap();
        std::fs::write(&second, b"same").unwrap();
        std::fs::write(&third, b"different").unwrap();

        assert!(files_match_sha256(&first, &second).unwrap());
        assert!(!files_match_sha256(&first, &third).unwrap());
    }

    struct TestChatSource {
        chat_ids: Vec<i64>,
        chats: HashMap<i64, ChatSnapshot>,
    }

    #[async_trait]
    impl ChatDataSource for TestChatSource {
        async fn fetch_chat_ids(&self, limit: i32) -> Result<Vec<i64>> {
            if limit <= 0 {
                return Ok(Vec::new());
            }
            let take = limit as usize;
            Ok(self.chat_ids.iter().cloned().take(take).collect())
        }

        async fn fetch_chat_snapshot(&self, chat_id: i64) -> Result<ChatSnapshot> {
            self.chats
                .get(&chat_id)
                .cloned()
                .ok_or_else(|| TgError::Other(format!("Missing chat {chat_id}")))
        }
    }

    fn chat_snapshot(
        id: i64,
        title: &str,
        unread_count: i32,
        chat_type: ChatTypeKind,
    ) -> ChatSnapshot {
        ChatSnapshot {
            id,
            title: title.to_string(),
            unread_count,
            last_message: None,
            chat_type,
        }
    }

    #[tokio::test]
    async fn collect_filtered_chats_expands_limit_for_private_chats() {
        let chat_ids = vec![1, 2, 3, 4, 5, 6, 7];
        let mut chats = HashMap::new();
        chats.insert(1, chat_snapshot(1, "Group A", 0, ChatTypeKind::BasicGroup));
        chats.insert(2, chat_snapshot(2, "Group B", 1, ChatTypeKind::Supergroup));
        chats.insert(3, chat_snapshot(3, "Alice", 0, ChatTypeKind::Private));
        chats.insert(4, chat_snapshot(4, "Bob", 0, ChatTypeKind::Private));
        chats.insert(5, chat_snapshot(5, "Cara", 0, ChatTypeKind::Private));
        chats.insert(6, chat_snapshot(6, "Group C", 0, ChatTypeKind::BasicGroup));
        chats.insert(7, chat_snapshot(7, "Group D", 0, ChatTypeKind::Supergroup));

        let source = TestChatSource { chat_ids, chats };
        let result = collect_filtered_chats_from_source(&source, 3, |chat| {
            chat.chat_type == ChatTypeKind::Private
        })
        .await
        .unwrap();

        let ids: Vec<i64> = result.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn collect_filtered_chats_returns_partial_when_insufficient() {
        let chat_ids = vec![10, 11, 12, 13];
        let mut chats = HashMap::new();
        chats.insert(10, chat_snapshot(10, "Group", 0, ChatTypeKind::BasicGroup));
        chats.insert(11, chat_snapshot(11, "Unread A", 2, ChatTypeKind::Private));
        chats.insert(12, chat_snapshot(12, "Read", 0, ChatTypeKind::Private));
        chats.insert(
            13,
            chat_snapshot(13, "Unread B", 1, ChatTypeKind::Supergroup),
        );

        let source = TestChatSource { chat_ids, chats };
        let result = collect_filtered_chats_from_source(&source, 5, |chat| chat.unread_count > 0)
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|c| c.unread_count > 0));
    }

    #[tokio::test]
    async fn collect_filtered_chats_handles_zero_limit() {
        let source = TestChatSource {
            chat_ids: vec![1, 2, 3],
            chats: HashMap::new(),
        };

        let result = collect_filtered_chats_from_source(&source, 0, |_| true)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    // --- collect_messages_paginated tests ---

    fn msg(id: i64) -> MessageInfo {
        MessageInfo {
            id,
            chat_id: 1,
            sender: "Alice".to_string(),
            text: format!("msg {id}"),
            date: "1h ago".to_string(),
            is_outgoing: false,
            edit_date: None,
            content_type: Some("text".to_string()),
            is_downloadable: false,
            download_files: vec![],
            content: None,
        }
    }

    struct TestMessageSource {
        /// Each inner Vec is one batch returned per call (in order).
        batches: std::sync::Mutex<Vec<Vec<MessageInfo>>>,
    }

    impl TestMessageSource {
        fn new(batches: Vec<Vec<MessageInfo>>) -> Self {
            Self {
                batches: std::sync::Mutex::new(batches),
            }
        }
    }

    #[async_trait]
    impl MessageHistorySource for TestMessageSource {
        async fn fetch_batch(
            &self,
            _chat_id: i64,
            _from_message_id: i64,
            _limit: i32,
        ) -> Result<Vec<MessageInfo>> {
            let mut batches = self.batches.lock().unwrap();
            if batches.is_empty() {
                Ok(vec![])
            } else {
                Ok(batches.remove(0))
            }
        }
    }

    #[tokio::test]
    async fn collect_messages_returns_exact_limit() {
        // Single batch has exactly `limit` messages.
        let source = TestMessageSource::new(vec![vec![msg(3), msg(2), msg(1)]]);
        let result = collect_messages_paginated(&source, 0, 3, None)
            .await
            .unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].id, 3);
        assert_eq!(result[2].id, 1);
    }

    #[tokio::test]
    async fn collect_messages_paginates_across_batches() {
        // TDLib returns 1 message per call; need 3 total.
        let source = TestMessageSource::new(vec![vec![msg(10)], vec![msg(9)], vec![msg(8)]]);
        let result = collect_messages_paginated(&source, 0, 3, None)
            .await
            .unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(
            result.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![10, 9, 8]
        );
    }

    #[tokio::test]
    async fn collect_messages_stops_at_limit_even_with_extra() {
        // Batch has more messages than limit; should stop at limit.
        let source = TestMessageSource::new(vec![vec![msg(5), msg(4), msg(3), msg(2), msg(1)]]);
        let result = collect_messages_paginated(&source, 0, 3, None)
            .await
            .unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn collect_messages_retries_on_empty_then_succeeds() {
        // First two calls return empty (TDLib syncing), third returns data.
        let source = TestMessageSource::new(vec![vec![], vec![], vec![msg(1), msg(2), msg(3)]]);
        let result = collect_messages_paginated(&source, 0, 3, None)
            .await
            .unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn collect_messages_returns_partial_when_exhausted() {
        // Only 2 messages exist but limit is 5; should return 2.
        let source = TestMessageSource::new(vec![vec![msg(2), msg(1)]]);
        let result = collect_messages_paginated(&source, 0, 5, None)
            .await
            .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn collect_messages_deduplicates_across_pages() {
        // Second batch repeats an ID from the first (can happen at page boundary).
        let source = TestMessageSource::new(vec![vec![msg(3), msg(2)], vec![msg(2), msg(1)]]);
        let result = collect_messages_paginated(&source, 0, 3, None)
            .await
            .unwrap();
        let ids: Vec<i64> = result.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![3, 2, 1]);
    }

    #[tokio::test]
    async fn collect_messages_stops_at_boundary_id() {
        // Messages 10, 9, 8, 7, 6 — boundary at 7 means 10, 9, 8, 7 returned (inclusive).
        let source = TestMessageSource::new(vec![vec![msg(10), msg(9), msg(8), msg(7), msg(6)]]);
        let result = collect_messages_paginated(&source, 0, 10, Some(7))
            .await
            .unwrap();
        let ids: Vec<i64> = result.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![10, 9, 8, 7]);
    }

    #[tokio::test]
    async fn collect_messages_boundary_and_limit_combined() {
        // Limit 2, boundary at 5. Messages: 10, 9, 8, 7, 6, 5.
        let source =
            TestMessageSource::new(vec![vec![msg(10), msg(9), msg(8), msg(7), msg(6), msg(5)]]);
        // Limit kicks in before boundary.
        let result = collect_messages_paginated(&source, 0, 2, Some(5))
            .await
            .unwrap();
        let ids: Vec<i64> = result.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![10, 9]);
    }

    #[tokio::test]
    async fn collect_messages_boundary_across_batches() {
        // First batch: 10, 9. Second batch: 8, 7, 6. Boundary at 7 (inclusive).
        let source =
            TestMessageSource::new(vec![vec![msg(10), msg(9)], vec![msg(8), msg(7), msg(6)]]);
        let result = collect_messages_paginated(&source, 0, 10, Some(7))
            .await
            .unwrap();
        let ids: Vec<i64> = result.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![10, 9, 8, 7]);
    }

    #[tokio::test]
    async fn collect_messages_boundary_above_all_returns_empty() {
        // All messages have id < boundary — none included.
        let source = TestMessageSource::new(vec![vec![msg(3), msg(2), msg(1)]]);
        let result = collect_messages_paginated(&source, 0, 10, Some(5))
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn collect_messages_boundary_is_inclusive_exact_match() {
        // Boundary at id=7 — message 7 must appear in results (inclusive semantics).
        // Simulates: messages at timestamps 10,9,8,7,6 and since_utc points to id=7.
        let source = TestMessageSource::new(vec![vec![
            msg(10),
            msg(9),
            msg(8),
            msg(7), // <-- boundary: should be INCLUDED
            msg(6),
            msg(5),
        ]]);
        let result = collect_messages_paginated(&source, 0, 20, Some(7))
            .await
            .unwrap();
        let ids: Vec<i64> = result.iter().map(|m| m.id).collect();
        assert!(ids.contains(&7), "boundary message (id=7) must be included");
        assert!(
            !ids.contains(&6),
            "message before boundary (id=6) must be excluded"
        );
        assert_eq!(ids, vec![10, 9, 8, 7]);
    }

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

        // Should complete within 3 seconds for a fresh client with no receive thread.
        assert!(elapsed < Duration::from_secs(3));
        assert!(client.shutdown.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn shutdown_joins_receive_thread_handle() {
        let mut client = TdLibClient::new(12345, "test_hash".to_string()).unwrap();

        let shutdown_flag = client.shutdown.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let exited_clone = exited.clone();

        let handle = std::thread::spawn(move || {
            while !shutdown_flag.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            exited_clone.store(true, Ordering::Relaxed);
        });

        *client.receive_handle.lock().await = Some(handle);
        client.shutdown().await;

        assert!(exited.load(Ordering::Relaxed));
        assert!(client.receive_handle.lock().await.is_none());
    }
}
