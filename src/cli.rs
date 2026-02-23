use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "tg")]
#[command(version, about = "A modern CLI tool for interacting with Telegram")]
pub struct Cli {
    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Authenticate with Telegram
    Auth(AuthArgs),

    /// Send a message to a contact or group
    Send(SendArgs),

    /// List 1:1 chats (direct messages only)
    Chats(ChatsArgs),

    /// List group chats only
    Groups(GroupsArgs),

    /// List all unread chats (1:1 and groups)
    Unread(UnreadArgs),

    /// Read messages from a chat
    Messages(MessagesArgs),

    /// Mark a chat as read
    MarkRead(MarkReadArgs),

    /// Mark a chat as unread
    MarkUnread(MarkUnreadArgs),

    /// Search contacts by name
    Search(SearchArgs),
}

#[derive(Parser, Debug)]
pub struct AuthArgs {
    /// Phone number (e.g., +1234567890)
    #[arg(long, env = "TG_PHONE")]
    pub phone: Option<String>,
}

#[derive(Parser, Debug)]
pub struct SendArgs {
    /// Contact name (required unless --id or --group is provided)
    #[arg(required_unless_present_any = ["id", "group"])]
    pub name: Option<String>,

    /// Message text
    #[arg(short, long)]
    pub message: String,

    /// Chat ID (for piping from search)
    #[arg(long, allow_hyphen_values = true)]
    pub id: Option<i64>,

    /// Group name to send to
    #[arg(long)]
    pub group: Option<String>,
}

#[derive(Parser, Debug)]
pub struct ChatsArgs {
    /// Maximum number of chats to list
    #[arg(long, default_value = "50")]
    pub limit: i32,
}

#[derive(Parser, Debug)]
pub struct GroupsArgs {
    /// Maximum number of groups to list
    #[arg(long, default_value = "50")]
    pub limit: i32,
}

#[derive(Parser, Debug)]
pub struct UnreadArgs {
    /// Maximum number of chats to list
    #[arg(long, default_value = "50")]
    pub limit: i32,
}

#[derive(Parser, Debug)]
pub struct MessagesArgs {
    /// Contact or group name
    #[arg(required_unless_present = "id")]
    pub name: Option<String>,

    /// Chat ID
    #[arg(long, allow_hyphen_values = true)]
    pub id: Option<i64>,

    /// Maximum number of messages to read
    #[arg(long, default_value = "20")]
    pub limit: i32,
}

#[derive(Parser, Debug)]
pub struct MarkReadArgs {
    /// Contact or group name
    #[arg(required_unless_present = "id")]
    pub name: Option<String>,

    /// Chat ID
    #[arg(long, allow_hyphen_values = true)]
    pub id: Option<i64>,
}

#[derive(Parser, Debug)]
pub struct MarkUnreadArgs {
    /// Contact or group name
    #[arg(required_unless_present = "id")]
    pub name: Option<String>,

    /// Chat ID
    #[arg(long, allow_hyphen_values = true)]
    pub id: Option<i64>,
}

#[derive(Parser, Debug)]
pub struct SearchArgs {
    /// Name to search for
    pub query: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_cli() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_auth_with_phone() {
        let cli = Cli::parse_from(["tg", "auth", "--phone", "+1234567890"]);
        match cli.command {
            Command::Auth(args) => {
                assert_eq!(args.phone, Some("+1234567890".to_string()));
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_send_by_name() {
        let cli = Cli::parse_from(["tg", "send", "John Doe", "-m", "Hello!"]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.name, Some("John Doe".to_string()));
                assert_eq!(args.id, None);
                assert_eq!(args.group, None);
                assert_eq!(args.message, "Hello!");
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_by_id() {
        let cli = Cli::parse_from(["tg", "send", "--id", "123456789", "-m", "Hello!"]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.name, None);
                assert_eq!(args.id, Some(123456789));
                assert_eq!(args.message, "Hello!");
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_to_group() {
        let cli = Cli::parse_from(["tg", "send", "--group", "Family Chat", "-m", "Hello everyone!"]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.name, None);
                assert_eq!(args.group, Some("Family Chat".to_string()));
                assert_eq!(args.message, "Hello everyone!");
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_chats_default_limit() {
        let cli = Cli::parse_from(["tg", "chats"]);
        match cli.command {
            Command::Chats(args) => {
                assert_eq!(args.limit, 50);
            }
            _ => panic!("Expected Chats command"),
        }
    }

    #[test]
    fn parse_chats_custom_limit() {
        let cli = Cli::parse_from(["tg", "chats", "--limit", "100"]);
        match cli.command {
            Command::Chats(args) => {
                assert_eq!(args.limit, 100);
            }
            _ => panic!("Expected Chats command"),
        }
    }

    #[test]
    fn parse_groups() {
        let cli = Cli::parse_from(["tg", "groups", "--limit", "25"]);
        match cli.command {
            Command::Groups(args) => {
                assert_eq!(args.limit, 25);
            }
            _ => panic!("Expected Groups command"),
        }
    }

    #[test]
    fn parse_unread() {
        let cli = Cli::parse_from(["tg", "unread"]);
        match cli.command {
            Command::Unread(args) => {
                assert_eq!(args.limit, 50);
            }
            _ => panic!("Expected Unread command"),
        }
    }

    #[test]
    fn parse_messages_by_name() {
        let cli = Cli::parse_from(["tg", "messages", "John Doe"]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.name, Some("John Doe".to_string()));
                assert_eq!(args.id, None);
                assert_eq!(args.limit, 20);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_by_id() {
        let cli = Cli::parse_from(["tg", "messages", "--id", "123456789", "--limit", "50"]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.name, None);
                assert_eq!(args.id, Some(123456789));
                assert_eq!(args.limit, 50);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_mark_read() {
        let cli = Cli::parse_from(["tg", "mark-read", "John Doe"]);
        match cli.command {
            Command::MarkRead(args) => {
                assert_eq!(args.name, Some("John Doe".to_string()));
                assert_eq!(args.id, None);
            }
            _ => panic!("Expected MarkRead command"),
        }
    }

    #[test]
    fn parse_mark_unread() {
        let cli = Cli::parse_from(["tg", "mark-unread", "--id", "123456789"]);
        match cli.command {
            Command::MarkUnread(args) => {
                assert_eq!(args.name, None);
                assert_eq!(args.id, Some(123456789));
            }
            _ => panic!("Expected MarkUnread command"),
        }
    }

    #[test]
    fn parse_search() {
        let cli = Cli::parse_from(["tg", "search", "John"]);
        match cli.command {
            Command::Search(args) => {
                assert_eq!(args.query, "John");
            }
            _ => panic!("Expected Search command"),
        }
    }

    #[test]
    fn parse_json_flag() {
        let cli = Cli::parse_from(["tg", "--json", "chats"]);
        assert!(cli.json);
    }

    #[test]
    fn parse_json_flag_after_command() {
        let cli = Cli::parse_from(["tg", "chats", "--json"]);
        assert!(cli.json);
    }

    // Negative ID tests (supergroups have negative IDs like -1001234567890)

    #[test]
    fn parse_messages_by_negative_id() {
        let cli = Cli::parse_from(["tg", "messages", "--id", "-1001666847309"]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.id, Some(-1001666847309));
                assert_eq!(args.name, None);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_send_by_negative_id() {
        let cli = Cli::parse_from(["tg", "send", "--id", "-1001234567890", "-m", "hi"]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.id, Some(-1001234567890));
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_mark_read_by_negative_id() {
        let cli = Cli::parse_from(["tg", "mark-read", "--id", "-1001234567890"]);
        match cli.command {
            Command::MarkRead(args) => {
                assert_eq!(args.id, Some(-1001234567890));
            }
            _ => panic!("Expected MarkRead command"),
        }
    }

    #[test]
    fn parse_mark_unread_by_negative_id() {
        let cli = Cli::parse_from(["tg", "mark-unread", "--id", "-1001234567890"]);
        match cli.command {
            Command::MarkUnread(args) => {
                assert_eq!(args.id, Some(-1001234567890));
            }
            _ => panic!("Expected MarkUnread command"),
        }
    }
}
