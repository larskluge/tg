use colored::Colorize;
use comfy_table::{Attribute, Cell, CellAlignment, ContentArrangement, Table};
use serde::{Deserialize, Serialize};
use std::env;
use terminal_size::terminal_size;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Plain,
    Json,
}

impl OutputFormat {
    pub fn from_json_flag(json: bool) -> Self {
        if json {
            OutputFormat::Json
        } else {
            OutputFormat::Plain
        }
    }
}

pub fn print_output<T: Serialize + PlainText>(format: OutputFormat, data: &T) {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(data).unwrap());
        }
        OutputFormat::Plain => {
            println!("{}", data.to_plain_text());
        }
    }
}

pub fn print_list<T: Serialize + PlainText>(format: OutputFormat, items: &[T]) {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(items).unwrap());
        }
        OutputFormat::Plain => {
            for item in items {
                println!("{}", item.to_plain_text());
            }
        }
    }
}

pub fn print_success(message: &str) {
    println!("{} {}", "✓".green(), message);
}

pub fn print_error(message: &str) {
    eprintln!("{} {}", "✗".red(), message);
}

pub trait PlainText {
    fn to_plain_text(&self) -> String;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatInfo {
    pub id: i64,
    pub name: String,
    pub unread_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
}

impl PlainText for ChatInfo {
    fn to_plain_text(&self) -> String {
        let unread = if self.unread_count > 0 {
            format!(" ({})", self.unread_count.to_string().yellow())
        } else {
            String::new()
        };
        format!("{}  {}{}", self.id, self.name.bold(), unread)
    }
}

fn terminal_width() -> usize {
    if let Some((terminal_size::Width(width), _)) = terminal_size() {
        return width as usize;
    }

    env::var("COLUMNS")
        .ok()
        .and_then(|cols| cols.parse::<usize>().ok())
        .filter(|cols| *cols > 0)
        .unwrap_or(80)
}

fn display_width(text: &str) -> usize {
    text.width()
}

fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                while let Some(next) = chars.next() {
                    if next == 'm' {
                        break;
                    }
                }
                continue;
            }
        }
        result.push(ch);
    }
    result
}

fn max_visible_width(text: &str) -> usize {
    text.lines()
        .map(|line| display_width(&strip_ansi(line)))
        .max()
        .unwrap_or(0)
}

fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    if max_width == 0 {
        return String::new();
    }

    let text_width = display_width(text);
    if text_width <= max_width {
        return text.to_string();
    }

    if max_width < 3 {
        return ".".repeat(max_width);
    }

    let target_width = max_width - 3;
    let mut result = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width > target_width {
            break;
        }
        result.push(ch);
        current_width += ch_width;
    }

    result.push_str("...");
    result
}

fn chats_table_overhead() -> usize {
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("H"),
        Cell::new("H"),
        Cell::new("H"),
        Cell::new("H"),
    ]);
    table.add_row(vec![
        Cell::new("x"),
        Cell::new("x"),
        Cell::new("x"),
        Cell::new("x"),
    ]);
    let rendered = table.to_string();
    max_visible_width(&rendered).saturating_sub(4)
}

/// Print a list of chats as a formatted table
pub fn print_chats_table(chats: &[ChatInfo]) {
    if chats.is_empty() {
        return;
    }

    let name_width = chats
        .iter()
        .map(|c| display_width(&c.name))
        .max()
        .unwrap_or(4)
        .max(display_width("Name"));
    let id_width = chats
        .iter()
        .map(|c| display_width(&c.id.to_string()))
        .max()
        .unwrap_or(7)
        .max(display_width("Chat ID"));
    let unread_width = chats
        .iter()
        .map(|c| display_width(&c.unread_count.to_string()))
        .max()
        .unwrap_or(6)
        .max(display_width("Unread"));

    let overhead = chats_table_overhead();
    let base_width = name_width + id_width + unread_width + overhead;
    // Add safety margin for emoji width calculation differences
    let available = terminal_width().saturating_sub(base_width + 4);
    let max_last_width = available.max(1);

    let last_header = truncate_with_ellipsis("Last message", max_last_width);

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    table.set_content_arrangement(ContentArrangement::Disabled);
    table.set_header(vec![
        Cell::new("Name")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Chat ID")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Unread")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new(last_header)
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
    ]);

    for chat in chats {
        let last_message = chat.last_message.as_deref().unwrap_or("-");
        let last_message = single_line(last_message);
        let last_message = truncate_with_ellipsis(&last_message, max_last_width);
        table.add_row(vec![
            Cell::new(&chat.name),
            Cell::new(chat.id),
            Cell::new(chat.unread_count),
            Cell::new(last_message),
        ]);
    }

    // Set right alignment for numeric columns and prevent wrapping
    for (index, column) in table.column_iter_mut().enumerate() {
        // Use a delimiter that won't appear in text to prevent wrapping
        column.set_delimiter('\0');
        if index == 1 || index == 2 {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }

    println!("{table}");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactInfo {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

impl PlainText for ContactInfo {
    fn to_plain_text(&self) -> String {
        // Single row format: Name | Username | Chat ID | Phone
        let username = self.username.as_deref().unwrap_or("-");
        let phone = self.phone.as_deref().unwrap_or("-");
        format!("{}  @{}  {}  {}", self.name, username, self.id, phone)
    }
}

/// Print a list of contacts as a formatted table
pub fn print_contacts_table(contacts: &[ContactInfo]) {
    if contacts.is_empty() {
        return;
    }

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    table.set_content_arrangement(ContentArrangement::Disabled);
    table.set_header(vec![
        Cell::new("Name")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Username")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Chat ID")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Phone")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
    ]);

    for contact in contacts {
        let username = contact
            .username
            .as_ref()
            .map(|u| format!("@{}", u))
            .unwrap_or_else(|| "-".to_string());
        let phone = contact.phone.as_deref().unwrap_or("-");
        table.add_row(vec![
            Cell::new(&contact.name),
            Cell::new(username),
            Cell::new(contact.id),
            Cell::new(phone),
        ]);
    }

    // Set right alignment for numeric column and prevent wrapping
    for (index, column) in table.column_iter_mut().enumerate() {
        column.set_delimiter('\0');
        if index == 2 {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }

    println!("{table}");
}

fn messages_table_string(messages: &[MessageInfo]) -> String {
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    table.set_content_arrangement(ContentArrangement::Disabled);
    table.set_header(vec![
        Cell::new("Message ID")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Timestamp")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Sender")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
        Cell::new("Message")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Underlined),
    ]);

    for message in messages {
        let sender = if message.is_outgoing {
            "You".to_string()
        } else {
            if let Some(id) = message.sender_id {
                format!("{} ({})", message.sender, id)
            } else {
                message.sender.clone()
            }
        };
        table.add_row(vec![
            Cell::new(message.id),
            Cell::new(&message.date),
            Cell::new(sender),
            Cell::new(single_line(&message.text)),
        ]);
    }

    // Right align message ID column and prevent wrapping.
    for (index, column) in table.column_iter_mut().enumerate() {
        column.set_delimiter('\0');
        if index == 0 {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }

    table.to_string()
}

/// Print a list of messages as a formatted table
pub fn print_messages_table(messages: &[MessageInfo]) {
    if messages.is_empty() {
        return;
    }
    println!("{}", messages_table_string(messages));
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageContentDetails {
    Text {
        text: String,
    },
    Photo {
        width: Option<i32>,
        height: Option<i32>,
        caption: Option<String>,
        has_spoiler: bool,
        is_secret: bool,
    },
    Video {
        width: i32,
        height: i32,
        duration_seconds: i32,
        caption: Option<String>,
        file_name: Option<String>,
        mime_type: Option<String>,
        has_spoiler: bool,
        is_secret: bool,
        supports_streaming: bool,
    },
    Document {
        caption: Option<String>,
        file_name: Option<String>,
        mime_type: Option<String>,
    },
    Sticker {
        emoji: Option<String>,
        width: i32,
        height: i32,
        format: String,
    },
    Audio {
        title: Option<String>,
        performer: Option<String>,
        duration_seconds: i32,
        caption: Option<String>,
        file_name: Option<String>,
        mime_type: Option<String>,
    },
    Voice {
        duration_seconds: i32,
        caption: Option<String>,
        mime_type: Option<String>,
        is_listened: bool,
    },
    Animation {
        width: i32,
        height: i32,
        duration_seconds: i32,
        caption: Option<String>,
        file_name: Option<String>,
        mime_type: Option<String>,
        has_spoiler: bool,
        is_secret: bool,
    },
    VideoNote {
        duration_seconds: i32,
        length: i32,
        is_viewed: bool,
        is_secret: bool,
    },
    Location {
        latitude: f64,
        longitude: f64,
        horizontal_accuracy: f64,
        live_period: i32,
        expires_in: i32,
        heading: i32,
        proximity_alert_radius: i32,
    },
    Contact {
        phone_number: String,
        first_name: String,
        last_name: Option<String>,
        user_id: i64,
        vcard: Option<String>,
    },
    Emoji {
        emoji: Option<String>,
        sticker_width: i32,
        sticker_height: i32,
        fitzpatrick_type: i32,
        sticker_format: Option<String>,
        custom_emoji_id: Option<i64>,
        has_sound: bool,
    },
    Poll {
        question: String,
        option_count: usize,
        total_voter_count: i32,
        is_anonymous: bool,
        is_closed: bool,
        poll_type: String,
    },
    Call {
        is_video: bool,
        discard_reason: String,
        duration_seconds: i32,
    },
    ContactRegistered {},
    Venue {
        title: String,
        address: String,
        latitude: f64,
        longitude: f64,
        provider: Option<String>,
    },
    PinMessage {
        pinned_message_id: i64,
    },
    GiftedPremium {
        gifter_user_id: i64,
        currency: String,
        amount: i64,
        month_count: i32,
    },
    Dice {
        emoji: String,
        value: i32,
    },
    Game {
        title: String,
        short_name: String,
        description: String,
    },
    Story {
        story_poster_chat_id: i64,
        story_id: i32,
        via_mention: bool,
    },
    Invoice {
        title: String,
        currency: String,
        total_amount: i64,
        is_test: bool,
    },
    VideoChatScheduled {
        group_call_id: i32,
        start_date: i32,
    },
    VideoChatStarted {
        group_call_id: i32,
    },
    VideoChatEnded {
        duration_seconds: i32,
    },
    InviteVideoChatParticipants {
        group_call_id: i32,
        user_ids: Vec<i64>,
    },
    BasicGroupChatCreate {
        title: String,
        member_user_ids: Vec<i64>,
    },
    SupergroupChatCreate {
        title: String,
    },
    ChatChangeTitle {
        title: String,
    },
    ChatChangePhoto {},
    ChatDeletePhoto {},
    ChatAddMembers {
        member_user_ids: Vec<i64>,
    },
    ChatJoinByLink {},
    ChatJoinByRequest {},
    ChatDeleteMember {
        user_id: i64,
    },
    ChatUpgradeTo {
        supergroup_id: i64,
    },
    ChatUpgradeFrom {
        title: String,
        basic_group_id: i64,
    },
    ScreenshotTaken {},
    ChatSetBackground {
        old_background_message_id: i64,
        only_for_self: bool,
    },
    ChatSetTheme {
        theme: Option<String>,
    },
    ChatSetMessageAutoDeleteTime {
        message_auto_delete_time: i32,
        from_user_id: i64,
    },
    ChatBoost {
        boost_count: i32,
    },
    ForumTopicCreated {
        name: String,
    },
    ForumTopicEdited {
        name: String,
        edit_icon_custom_emoji_id: bool,
        icon_custom_emoji_id: i64,
    },
    ForumTopicIsClosedToggled {
        is_closed: bool,
    },
    ForumTopicIsHiddenToggled {
        is_hidden: bool,
    },
    SuggestProfilePhoto {},
    CustomServiceAction {
        text: String,
    },
    GameScore {
        game_message_id: i64,
        game_id: i64,
        score: i32,
    },
    PaymentSuccessful {
        invoice_chat_id: i64,
        invoice_message_id: i64,
        currency: String,
        total_amount: i64,
        is_recurring: bool,
        invoice_name: Option<String>,
    },
    PremiumGiftCode {
        is_from_giveaway: bool,
        is_unclaimed: bool,
        currency: String,
        amount: i64,
        month_count: i32,
        code: String,
    },
    Giveaway {
        winner_count: i32,
    },
    GiveawayCompleted {
        giveaway_message_id: i64,
        winner_count: i32,
        unclaimed_prize_count: i32,
    },
    GiveawayCreated {},
    GiveawayWinners {
        boosted_chat_id: i64,
        giveaway_message_id: i64,
        winner_count: i32,
        winner_user_ids: Vec<i64>,
        unclaimed_prize_count: i32,
    },
    UsersShared {
        button_id: i32,
    },
    ChatShared {
        button_id: i32,
    },
    BotWriteAccessAllowed {},
    WebAppDataSent {
        button_text: String,
    },
    PassportDataSent {},
    ProximityAlertTriggered {
        distance: i32,
    },
    ExpiredPhoto {},
    ExpiredVideo {},
    ExpiredVideoNote {},
    ExpiredVoiceNote {},
    GroupCall {
        is_video: bool,
        duration: i32,
    },
    GiftedStars {
        gifter_user_id: i64,
        receiver_user_id: i64,
        star_count: i64,
    },
    PaidMedia {
        star_count: i64,
        caption: Option<String>,
    },
    Gift {},
    GiveawayPrizeStars {
        star_count: i64,
        giveaway_message_id: i64,
    },
    ChatOwnerChanged {
        new_owner_user_id: i64,
    },
    ChatOwnerLeft {
        new_owner_user_id: i64,
    },
    PaymentRefunded {
        currency: String,
        total_amount: i64,
    },
    Unsupported {
        tdlib_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageFileRef {
    pub file_id: i32,
    pub is_primary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    pub expected_size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_unique_id: Option<String>,
    pub can_be_downloaded: bool,
    pub is_downloaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: i64,
    pub chat_id: i64,
    pub sender_id: Option<i64>,
    pub sender: String,
    pub text: String,
    pub date: String,
    #[serde(skip)]
    pub timestamp: i32,
    pub is_outgoing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub is_downloadable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub download_files: Vec<MessageFileRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContentDetails>,
}

impl PlainText for MessageInfo {
    fn to_plain_text(&self) -> String {
        let sender = if self.is_outgoing {
            "You".blue().to_string()
        } else {
            if let Some(id) = self.sender_id {
                format!("{} ({})", self.sender.green(), id)
            } else {
                self.sender.green().to_string()
            }
        };
        format!("[{}] {}: {}", self.date.dimmed(), sender, self.text)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Downloaded,
    SkippedSameHash,
    RenamedConflict,
    Failed,
    NoDownloadableMedia,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedFileResult {
    pub file_id: i32,
    pub is_primary: bool,
    pub status: DownloadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    pub expected_size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadReport {
    pub chat_id: i64,
    pub message_id: i64,
    pub status: DownloadStatus,
    pub output_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContentDetails>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<DownloadedFileResult>,
}

impl PlainText for DownloadReport {
    fn to_plain_text(&self) -> String {
        let mut lines = vec![
            format!(
                "chat={} message={} status={:?}",
                self.chat_id, self.message_id, self.status
            ),
            format!("output={}", self.output_dir),
        ];

        for file in &self.files {
            let mut line = format!("file_id={} status={:?}", file.file_id, file.status);
            if let Some(path) = &file.saved_path {
                line.push_str(&format!(" path={path}"));
            }
            if let Some(name) = &file.file_name {
                line.push_str(&format!(" name={name}"));
            }
            lines.push(line);
        }

        lines.join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

impl PlainText for UserInfo {
    fn to_plain_text(&self) -> String {
        let mut parts = vec![format!("ID: {}", self.id)];
        let name = if self.last_name.is_empty() {
            self.first_name.clone()
        } else {
            format!("{} {}", self.first_name, self.last_name)
        };
        parts.push(format!("Name: {}", name));
        if let Some(ref username) = self.username {
            parts.push(format!("Username: @{}", username));
        }
        if let Some(ref phone) = self.phone {
            parts.push(format!("Phone: {}", phone));
        }
        parts.join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResult {
    pub message_id: i64,
    pub chat_id: i64,
}

impl PlainText for SendResult {
    fn to_plain_text(&self) -> String {
        format!("Message sent (id: {})", self.message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_format_from_json_flag() {
        assert_eq!(OutputFormat::from_json_flag(true), OutputFormat::Json);
        assert_eq!(OutputFormat::from_json_flag(false), OutputFormat::Plain);
    }

    #[test]
    fn chat_info_plain_text() {
        let chat = ChatInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            unread_count: 0,
            last_message: None,
        };
        let text = chat.to_plain_text();
        assert!(text.contains("123456789"));
        assert!(text.contains("John Doe"));
    }

    #[test]
    fn chat_info_plain_text_with_unread() {
        let chat = ChatInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            unread_count: 5,
            last_message: None,
        };
        let text = chat.to_plain_text();
        assert!(text.contains("123456789"));
        assert!(text.contains("John Doe"));
        assert!(text.contains("5"));
    }

    #[test]
    fn chat_info_json() {
        let chat = ChatInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            unread_count: 5,
            last_message: Some("Hello".to_string()),
        };
        let json = serde_json::to_string(&chat).unwrap();
        assert!(json.contains("\"id\":123456789"));
        assert!(json.contains("\"name\":\"John Doe\""));
        assert!(json.contains("\"unread_count\":5"));
        assert!(json.contains("\"last_message\":\"Hello\""));
    }

    #[test]
    fn chat_info_json_no_last_message() {
        let chat = ChatInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            unread_count: 0,
            last_message: None,
        };
        let json = serde_json::to_string(&chat).unwrap();
        assert!(!json.contains("last_message"));
    }

    #[test]
    fn contact_info_plain_text() {
        let contact = ContactInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            username: Some("johndoe".to_string()),
            phone: Some("+1234567890".to_string()),
        };
        let text = contact.to_plain_text();
        assert!(text.contains("John Doe"));
        assert!(text.contains("@johndoe"));
        assert!(text.contains("123456789"));
        assert!(text.contains("+1234567890"));
    }

    #[test]
    fn contact_info_plain_text_no_optional_fields() {
        let contact = ContactInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            username: None,
            phone: None,
        };
        let text = contact.to_plain_text();
        assert!(text.contains("John Doe"));
        assert!(text.contains("123456789"));
        assert!(text.contains("-")); // placeholder for missing fields
    }

    #[test]
    fn contact_info_json() {
        let contact = ContactInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            username: Some("johndoe".to_string()),
            phone: Some("+1234567890".to_string()),
        };
        let json = serde_json::to_string(&contact).unwrap();
        assert!(json.contains("\"id\":123456789"));
        assert!(json.contains("\"name\":\"John Doe\""));
        assert!(json.contains("\"username\":\"johndoe\""));
        assert!(json.contains("\"phone\":\"+1234567890\""));
    }

    #[test]
    fn contact_info_json_no_optional_fields() {
        let contact = ContactInfo {
            id: 123456789,
            name: "John Doe".to_string(),
            username: None,
            phone: None,
        };
        let json = serde_json::to_string(&contact).unwrap();
        assert!(json.contains("\"id\":123456789"));
        assert!(json.contains("\"name\":\"John Doe\""));
        assert!(!json.contains("username")); // should be skipped
        assert!(!json.contains("phone")); // should be skipped
    }

    #[test]
    fn message_info_plain_text_incoming() {
        let msg = MessageInfo {
            id: 1,
            chat_id: 123,
            sender_id: Some(400),
            sender: "John".to_string(),
            text: "Hello!".to_string(),
            date: "2024-01-01 12:00".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: None,
            content_type: None,
            is_downloadable: false,
            download_files: vec![],
            content: None,
        };
        let text = msg.to_plain_text();
        assert!(text.contains("2024-01-01 12:00"));
        assert!(text.contains("John"));
        assert!(text.contains("(400)"));
        assert!(text.contains("Hello!"));
    }

    #[test]
    fn message_info_plain_text_outgoing() {
        let msg = MessageInfo {
            id: 1,
            chat_id: 123,
            sender_id: Some(500),
            sender: "Me".to_string(),
            text: "Hi there!".to_string(),
            date: "2024-01-01 12:00".to_string(),
            timestamp: 0,
            is_outgoing: true,
            edit_date: None,
            content_type: None,
            is_downloadable: false,
            download_files: vec![],
            content: None,
        };
        let text = msg.to_plain_text();
        assert!(text.contains("You"));
        assert!(text.contains("Hi there!"));
    }

    #[test]
    fn print_messages_table_renders_without_panic() {
        let msgs = vec![MessageInfo {
            id: 1,
            chat_id: 123,
            sender_id: Some(400),
            sender: "John".to_string(),
            text: "[Photo: 720x1280]".to_string(),
            date: "2026-02-25T17:45:12Z".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: None,
            content_type: Some("photo".to_string()),
            is_downloadable: true,
            download_files: vec![],
            content: None,
        }];

        print_messages_table(&msgs);
    }

    #[test]
    fn messages_table_contains_expected_columns_and_values() {
        let msgs = vec![MessageInfo {
            id: 42,
            chat_id: 123,
            sender_id: Some(300),
            sender: "Alice".to_string(),
            text: "[Emoji: 😀]".to_string(),
            date: "2026-02-25T17:45:12Z".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: None,
            content_type: Some("emoji".to_string()),
            is_downloadable: true,
            download_files: vec![],
            content: None,
        }];

        let table = messages_table_string(&msgs);
        assert!(table.contains("Message ID"));
        assert!(table.contains("Timestamp"));
        assert!(table.contains("Sender"));
        assert!(table.contains("Message"));
        assert!(table.contains("42"));
        assert!(table.contains("2026-02-25T17:45:12Z"));
        assert!(table.contains("Alice (300)"));
        assert!(table.contains("[Emoji: 😀]"));
    }

    #[test]
    fn message_info_json_includes_additive_fields() {
        let msg = MessageInfo {
            id: 1,
            chat_id: 123,
            sender_id: Some(300),
            sender: "Alice".to_string(),
            text: "Audio: Song".to_string(),
            date: "1h ago".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: None,
            content_type: Some("audio".to_string()),
            is_downloadable: true,
            download_files: vec![MessageFileRef {
                file_id: 10,
                is_primary: true,
                role: Some("main".to_string()),
                file_name: Some("song.mp3".to_string()),
                mime_type: Some("audio/mpeg".to_string()),
                size_bytes: 100,
                expected_size_bytes: 100,
                local_path: Some("/tmp/song.mp3".to_string()),
                remote_id: Some("remote".to_string()),
                remote_unique_id: Some("uniq".to_string()),
                can_be_downloaded: true,
                is_downloaded: true,
            }],
            content: Some(MessageContentDetails::Audio {
                title: Some("Song".to_string()),
                performer: Some("Artist".to_string()),
                duration_seconds: 120,
                caption: None,
                file_name: Some("song.mp3".to_string()),
                mime_type: Some("audio/mpeg".to_string()),
            }),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"chat_id\":123"));
        assert!(json.contains("\"sender_id\":300"));
        assert!(json.contains("\"content_type\":\"audio\""));
        assert!(json.contains("\"is_downloadable\":true"));
        assert!(json.contains("\"download_files\""));
        assert!(json.contains("\"kind\":\"audio\""));
    }

    #[test]
    fn anonymous_sender_has_null_sender_id_in_json() {
        let msg = MessageInfo {
            id: 1,
            chat_id: -1001234567890,
            sender_id: None,
            sender: "Tech Channel".to_string(),
            text: "Channel announcement".to_string(),
            date: "2024-01-01 12:00".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: None,
            content_type: None,
            is_downloadable: false,
            download_files: vec![],
            content: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"sender_id\":null"));
        assert!(json.contains("\"sender\":\"Tech Channel\""));
    }

    #[test]
    fn anonymous_sender_plain_text_omits_id() {
        let msg = MessageInfo {
            id: 1,
            chat_id: -1001234567890,
            sender_id: None,
            sender: "Tech Channel".to_string(),
            text: "Channel announcement".to_string(),
            date: "2024-01-01 12:00".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: None,
            content_type: None,
            is_downloadable: false,
            download_files: vec![],
            content: None,
        };
        let text = msg.to_plain_text();
        assert!(text.contains("Tech Channel"));
        assert!(!text.contains("("));
    }

    #[test]
    fn download_report_json_contains_absolute_paths() {
        let report = DownloadReport {
            chat_id: 123,
            message_id: 456,
            status: DownloadStatus::Downloaded,
            output_dir: "/tmp".to_string(),
            content_type: Some("video".to_string()),
            content: Some(MessageContentDetails::Video {
                width: 1920,
                height: 1080,
                duration_seconds: 42,
                caption: None,
                file_name: Some("clip.mp4".to_string()),
                mime_type: Some("video/mp4".to_string()),
                has_spoiler: false,
                is_secret: false,
                supports_streaming: true,
            }),
            files: vec![DownloadedFileResult {
                file_id: 99,
                is_primary: true,
                status: DownloadStatus::Downloaded,
                role: Some("main".to_string()),
                file_name: Some("clip.mp4".to_string()),
                mime_type: Some("video/mp4".to_string()),
                size_bytes: 12,
                expected_size_bytes: 12,
                source_path: Some("/var/cache/clip.mp4".to_string()),
                saved_path: Some("/tmp/clip.mp4".to_string()),
                note: None,
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"output_dir\":\"/tmp\""));
        assert!(json.contains("\"saved_path\":\"/tmp/clip.mp4\""));
        assert!(json.contains("\"kind\":\"video\""));
        assert!(json.contains("\"width\":1920"));
    }

    #[test]
    fn message_info_json_edit_date_none_is_omitted() {
        let msg = MessageInfo {
            id: 1,
            chat_id: 123,
            sender_id: Some(300),
            sender: "Alice".to_string(),
            text: "Hello".to_string(),
            date: "2024-01-01T00:00:00Z".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: None,
            content_type: None,
            is_downloadable: false,
            download_files: vec![],
            content: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("edit_date"));
    }

    #[test]
    fn message_info_json_edit_date_some_is_included() {
        let msg = MessageInfo {
            id: 1,
            chat_id: 123,
            sender_id: Some(300),
            sender: "Alice".to_string(),
            text: "Hello (edited)".to_string(),
            date: "2024-01-01T00:00:00Z".to_string(),
            timestamp: 0,
            is_outgoing: false,
            edit_date: Some("2024-01-01T00:05:00Z".to_string()),
            content_type: None,
            is_downloadable: false,
            download_files: vec![],
            content: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"edit_date\":\"2024-01-01T00:05:00Z\""));
    }

    #[test]
    fn send_result_plain_text() {
        let result = SendResult {
            message_id: 12345,
            chat_id: 67890,
        };
        assert_eq!(result.to_plain_text(), "Message sent (id: 12345)");
    }

    #[test]
    fn send_result_json() {
        let result = SendResult {
            message_id: 12345,
            chat_id: 67890,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"message_id\":12345"));
        assert!(json.contains("\"chat_id\":67890"));
    }

    #[test]
    fn single_line_replaces_controls() {
        let text = "hello\nworld\ttest\rline";
        let normalized = single_line(text);
        assert_eq!(normalized, "hello world test line");
    }

    #[test]
    fn truncate_with_ellipsis_shortens() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        assert_eq!(truncate_with_ellipsis(text, 10), "abcdefg...");
        assert_eq!(truncate_with_ellipsis(text, 3), "...");
        assert_eq!(truncate_with_ellipsis(text, 2), "..");
        assert_eq!(truncate_with_ellipsis(text, 1), ".");
    }

    #[test]
    fn truncate_with_ellipsis_no_change_when_fits() {
        let text = "short";
        assert_eq!(truncate_with_ellipsis(text, 5), "short");
        assert_eq!(truncate_with_ellipsis(text, 10), "short");
    }

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        let text = "\u{1b}[31mred\u{1b}[0m";
        assert_eq!(strip_ansi(text), "red");
    }

    #[test]
    fn max_visible_width_ignores_ansi_and_lines() {
        let text = "short\n\u{1b}[32mverylong\u{1b}[0m";
        assert_eq!(max_visible_width(text), 8);
    }
}
