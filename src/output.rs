use colored::Colorize;
use comfy_table::{Attribute, Cell, ContentArrangement, Table};
use serde::Serialize;
use std::env;
use terminal_size::terminal_size;

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

#[derive(Debug, Clone, Serialize)]
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
    text.chars().count()
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
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect()
}

fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
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

    let take = max_width - 3;
    let mut result = text.chars().take(take).collect::<String>();
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
    let available = terminal_width().saturating_sub(base_width);
    let max_last_width = available.max(1);

    let last_header = truncate_with_ellipsis("Last message", max_last_width);

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    table.set_content_arrangement(ContentArrangement::Dynamic);
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

    println!("{table}");
}

#[derive(Debug, Clone, Serialize)]
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
        format!(
            "{}  @{}  {}  {}",
            self.name, username, self.id, phone
        )
    }
}

/// Print a list of contacts as a formatted table
pub fn print_contacts_table(contacts: &[ContactInfo]) {
    if contacts.is_empty() {
        return;
    }

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    table.set_content_arrangement(ContentArrangement::Dynamic);
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

    println!("{table}");
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageInfo {
    pub id: i64,
    pub sender: String,
    pub text: String,
    pub date: String,
    pub is_outgoing: bool,
}

impl PlainText for MessageInfo {
    fn to_plain_text(&self) -> String {
        let sender = if self.is_outgoing {
            "You".blue().to_string()
        } else {
            self.sender.green().to_string()
        };
        format!("[{}] {}: {}", self.date.dimmed(), sender, self.text)
    }
}

#[derive(Debug, Clone, Serialize)]
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
            sender: "John".to_string(),
            text: "Hello!".to_string(),
            date: "2024-01-01 12:00".to_string(),
            is_outgoing: false,
        };
        let text = msg.to_plain_text();
        assert!(text.contains("2024-01-01 12:00"));
        assert!(text.contains("John"));
        assert!(text.contains("Hello!"));
    }

    #[test]
    fn message_info_plain_text_outgoing() {
        let msg = MessageInfo {
            id: 1,
            sender: "Me".to_string(),
            text: "Hi there!".to_string(),
            date: "2024-01-01 12:00".to_string(),
            is_outgoing: true,
        };
        let text = msg.to_plain_text();
        assert!(text.contains("You"));
        assert!(text.contains("Hi there!"));
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
