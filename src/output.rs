use colored::Colorize;
use serde::Serialize;

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

    // Calculate column widths
    let name_width = contacts.iter().map(|c| c.name.len()).max().unwrap_or(4).max(4);
    let username_width = contacts
        .iter()
        .map(|c| c.username.as_ref().map(|u| u.len() + 1).unwrap_or(1)) // +1 for @
        .max()
        .unwrap_or(8)
        .max(8);
    let id_width = contacts
        .iter()
        .map(|c| c.id.to_string().len())
        .max()
        .unwrap_or(7)
        .max(7);
    let phone_width = contacts
        .iter()
        .map(|c| c.phone.as_ref().map(|p| p.len()).unwrap_or(1))
        .max()
        .unwrap_or(5)
        .max(5);

    // Print header
    println!(
        "{:<name_width$}  {:<username_width$}  {:>id_width$}  {:<phone_width$}",
        "Name".bold(),
        "Username".bold(),
        "Chat ID".bold(),
        "Phone".bold(),
        name_width = name_width,
        username_width = username_width,
        id_width = id_width,
        phone_width = phone_width,
    );
    println!(
        "{:-<name_width$}  {:-<username_width$}  {:-<id_width$}  {:-<phone_width$}",
        "",
        "",
        "",
        "",
        name_width = name_width,
        username_width = username_width,
        id_width = id_width,
        phone_width = phone_width,
    );

    // Print rows
    for contact in contacts {
        let username = contact
            .username
            .as_ref()
            .map(|u| format!("@{}", u))
            .unwrap_or_else(|| "-".to_string());
        let phone = contact.phone.as_deref().unwrap_or("-");
        println!(
            "{:<name_width$}  {:<username_width$}  {:>id_width$}  {:<phone_width$}",
            contact.name,
            username.dimmed(),
            contact.id,
            phone.dimmed(),
            name_width = name_width,
            username_width = username_width,
            id_width = id_width,
            phone_width = phone_width,
        );
    }
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
}
