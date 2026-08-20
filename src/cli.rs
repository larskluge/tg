use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    #[command(
        about = "Authenticate with Telegram",
        long_about = "Authenticate with Telegram and create/update your local session.\n\nRun `tg auth` to start an interactive login flow. You will be prompted for:\n  1. API ID and API hash (from my.telegram.org) — unless already stored or set via TG_API_ID/TG_API_HASH\n  2. Phone number — unless set via TG_PHONE\n  3. Verification code sent to your Telegram app\n  4. 2FA password (if enabled on your account)\n\nOutcome: on success, tg stores your authenticated session and API credentials in the OS data directory (<data_dir>/tg), typically ~/Library/Application Support/tg on macOS and ~/.local/share/tg on Linux, so later commands do not require environment variables.",
        after_help = "Examples:\n  tg auth\n  TG_API_ID=12345 TG_API_HASH=abcdef TG_PHONE=+1234567890 tg auth\n\nEnvironment (optional, will prompt if not set):\n  TG_API_ID    Telegram API ID from my.telegram.org\n  TG_API_HASH  Telegram API hash from my.telegram.org\n  TG_PHONE     Phone number in E.164 format (e.g. +1234567890)"
    )]
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

    /// Download media from a single message
    Download(DownloadArgs),

    /// Mark a chat as read
    MarkRead(MarkReadArgs),

    /// Mark a chat as unread
    MarkUnread(MarkUnreadArgs),

    /// Search contacts by name
    Search(SearchArgs),

    /// Bulk-sync messages for multiple chats (reads chat_id:HWM map from stdin JSON)
    Sync(SyncArgs),

    /// Show your Telegram user info (ID, name, username, phone)
    Whoami,

    /// Run a long-lived TDLib server on a Unix socket so other `tg` commands skip cold start
    Serve,
}

#[derive(Parser, Debug)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub subcommand: Option<AuthSubcommand>,
}

#[derive(Subcommand, Debug)]
pub enum AuthSubcommand {
    /// Authenticate a bot with its token
    Bot(AuthBotArgs),

    /// Show current authentication status and configured bots
    Status,
}

#[derive(Parser, Debug)]
pub struct AuthBotArgs {
    /// Bot token from @BotFather (or set TG_BOT_TOKEN env var)
    #[arg(long, env = "TG_BOT_TOKEN")]
    pub token: Option<String>,
}

#[derive(Parser, Debug)]
pub struct SendArgs {
    /// Contact name (required unless --id, --to, or --group is provided)
    #[arg(required_unless_present_any = ["id", "group", "to"])]
    pub name: Option<String>,

    /// Message text (if omitted, read from stdin)
    #[arg(short, long)]
    pub message: Option<String>,

    /// Chat ID (for piping from search)
    #[arg(long, allow_hyphen_values = true)]
    pub id: Option<i64>,

    /// Recipient by @username or numeric ID
    #[arg(long, allow_hyphen_values = true)]
    pub to: Option<String>,

    /// Send as a bot (@username or numeric ID, must be authenticated via `tg auth bot`)
    #[arg(long = "as")]
    pub send_as: Option<String>,

    /// Group name to send to
    #[arg(long)]
    pub group: Option<String>,

    /// Format the message body: `HTML` or `MarkdownV2` (default: plain text)
    #[arg(long)]
    pub parse_mode: Option<String>,
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
    #[arg(required_unless_present = "chat")]
    pub name: Option<String>,

    /// Chat ID
    #[arg(long, allow_hyphen_values = true)]
    pub chat: Option<i64>,

    /// Maximum number of messages to read
    #[arg(long, default_value = "20")]
    pub limit: i32,

    /// Only include messages since this UTC date (YYYY-MM-DD or ISO 8601, e.g. 2026-03-18T09:34:05Z)
    #[arg(long)]
    pub since_utc: Option<String>,

    /// Return messages in chronological order (oldest first), fetching full history
    #[arg(long)]
    pub oldest_first: bool,
}

#[derive(Parser, Debug)]
pub struct DownloadArgs {
    /// Chat ID (required). Supports negative IDs for supergroups/channels.
    #[arg(long, allow_hyphen_values = true)]
    pub chat: i64,

    /// Message ID to download media from
    #[arg(long, allow_hyphen_values = true)]
    pub message: i64,

    /// Directory to save downloaded files to
    #[arg(long, default_value = ".")]
    pub output_dir: PathBuf,

    /// Download priority (1-32). Higher values are downloaded sooner when multiple downloads are queued.
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(i32).range(1..=32))]
    pub priority: i32,
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

#[derive(Parser, Debug)]
pub struct SyncArgs {
    /// Override all HWMs with now minus N days (for reconciliation sweeps)
    #[arg(long)]
    pub reconcile_days: Option<u32>,

    /// Maximum messages per chat
    #[arg(long, default_value = "1000")]
    pub limit: i32,
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
    fn parse_auth() {
        let cli = Cli::parse_from(["tg", "auth"]);
        assert!(matches!(cli.command, Command::Auth(_)));
    }

    #[test]
    fn auth_help_documents_flow_and_env() {
        let mut cmd = Cli::command();
        let mut help_buf = Vec::new();

        cmd.find_subcommand_mut("auth")
            .expect("auth subcommand should exist")
            .write_long_help(&mut help_buf)
            .expect("writing help should succeed");

        let help = String::from_utf8(help_buf).expect("help should be valid utf-8");

        assert!(help.contains("Verification code"));
        assert!(help.contains("TG_API_ID"));
        assert!(help.contains("TG_API_HASH"));
        assert!(help.contains("Examples:"));
        assert!(help.contains("Outcome:"));
        assert!(help.contains("OS data directory"));
        assert!(help.contains("do not require environment variables"));
        assert!(!help.contains("[env:"));
    }

    #[test]
    fn parse_send_by_name() {
        let cli = Cli::parse_from(["tg", "send", "John Doe", "-m", "Hello!"]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.name, Some("John Doe".to_string()));
                assert_eq!(args.id, None);
                assert_eq!(args.group, None);
                assert_eq!(args.message.as_deref(), Some("Hello!"));
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_with_parse_mode() {
        let cli = Cli::parse_from([
            "tg",
            "send",
            "--to",
            "@x",
            "--parse-mode",
            "HTML",
            "-m",
            "hi",
        ]);
        match cli.command {
            Command::Send(args) => assert_eq!(args.parse_mode.as_deref(), Some("HTML")),
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_without_parse_mode_is_none() {
        let cli = Cli::parse_from(["tg", "send", "John Doe", "-m", "Hello!"]);
        match cli.command {
            Command::Send(args) => assert_eq!(args.parse_mode, None),
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_as_bot_with_parse_mode() {
        // No clap conflict: the bot HTTP path honours the mode too.
        let cli = Cli::parse_from([
            "tg",
            "send",
            "--as",
            "@mybot",
            "--to",
            "@someone",
            "--parse-mode",
            "HTML",
            "-m",
            "hi",
        ]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.send_as.as_deref(), Some("@mybot"));
                assert_eq!(args.parse_mode.as_deref(), Some("HTML"));
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
                assert_eq!(args.message.as_deref(), Some("Hello!"));
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_to_group() {
        let cli = Cli::parse_from([
            "tg",
            "send",
            "--group",
            "Family Chat",
            "-m",
            "Hello everyone!",
        ]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.name, None);
                assert_eq!(args.group, Some("Family Chat".to_string()));
                assert_eq!(args.message.as_deref(), Some("Hello everyone!"));
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
                assert_eq!(args.chat, None);
                assert_eq!(args.limit, 20);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_by_id() {
        let cli = Cli::parse_from(["tg", "messages", "--chat", "123456789", "--limit", "50"]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.name, None);
                assert_eq!(args.chat, Some(123456789));
                assert_eq!(args.limit, 50);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_download_defaults() {
        let cli = Cli::parse_from(["tg", "download", "--chat", "123456789", "--message", "42"]);
        match cli.command {
            Command::Download(args) => {
                assert_eq!(args.chat, 123456789);
                assert_eq!(args.message, 42);
                assert_eq!(args.output_dir, PathBuf::from("."));
                assert_eq!(args.priority, 16);
            }
            _ => panic!("Expected Download command"),
        }
    }

    #[test]
    fn parse_download_negative_chat_id() {
        let cli = Cli::parse_from([
            "tg",
            "download",
            "--chat",
            "-1001666847309",
            "--message",
            "42",
        ]);
        match cli.command {
            Command::Download(args) => {
                assert_eq!(args.chat, -1001666847309);
                assert_eq!(args.message, 42);
            }
            _ => panic!("Expected Download command"),
        }
    }

    #[test]
    fn parse_download_custom_args() {
        let cli = Cli::parse_from([
            "tg",
            "download",
            "--chat",
            "123456789",
            "--message",
            "42",
            "--output-dir",
            "/tmp/tg-downloads",
            "--priority",
            "32",
        ]);
        match cli.command {
            Command::Download(args) => {
                assert_eq!(args.chat, 123456789);
                assert_eq!(args.message, 42);
                assert_eq!(args.output_dir, PathBuf::from("/tmp/tg-downloads"));
                assert_eq!(args.priority, 32);
            }
            _ => panic!("Expected Download command"),
        }
    }

    #[test]
    fn parse_download_rejects_low_priority() {
        let cli = Cli::try_parse_from([
            "tg",
            "download",
            "--chat",
            "123456789",
            "--message",
            "42",
            "--priority",
            "0",
        ]);
        assert!(cli.is_err());
    }

    #[test]
    fn parse_download_rejects_high_priority() {
        let cli = Cli::try_parse_from([
            "tg",
            "download",
            "--chat",
            "123456789",
            "--message",
            "42",
            "--priority",
            "33",
        ]);
        assert!(cli.is_err());
    }

    #[test]
    fn parse_download_rejects_repeated_message_flag() {
        let cli = Cli::try_parse_from([
            "tg",
            "download",
            "--chat",
            "123456789",
            "--message",
            "42",
            "--message",
            "43",
        ]);
        assert!(cli.is_err());
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
        let cli = Cli::parse_from(["tg", "messages", "--chat", "-1001666847309"]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.chat, Some(-1001666847309));
                assert_eq!(args.name, None);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_with_since_utc() {
        let cli = Cli::parse_from([
            "tg",
            "messages",
            "--chat",
            "123456789",
            "--since-utc",
            "2026-03-01",
        ]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.since_utc, Some("2026-03-01".to_string()));
                assert_eq!(args.limit, 20);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_with_since_utc_and_limit() {
        let cli = Cli::parse_from([
            "tg",
            "messages",
            "--chat",
            "123456789",
            "--since-utc",
            "2026-03-01",
            "--limit",
            "5",
        ]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.since_utc, Some("2026-03-01".to_string()));
                assert_eq!(args.limit, 5);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_without_since_utc() {
        let cli = Cli::parse_from(["tg", "messages", "John Doe"]);
        match cli.command {
            Command::Messages(args) => {
                assert_eq!(args.since_utc, None);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_oldest_first() {
        let cli = Cli::parse_from(["tg", "messages", "--chat", "123", "--oldest-first"]);
        match cli.command {
            Command::Messages(args) => {
                assert!(args.oldest_first);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_oldest_first_with_limit() {
        let cli = Cli::parse_from([
            "tg",
            "messages",
            "--chat",
            "123",
            "--oldest-first",
            "--limit",
            "1000",
        ]);
        match cli.command {
            Command::Messages(args) => {
                assert!(args.oldest_first);
                assert_eq!(args.limit, 1000);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_without_oldest_first() {
        let cli = Cli::parse_from(["tg", "messages", "--chat", "123"]);
        match cli.command {
            Command::Messages(args) => {
                assert!(!args.oldest_first);
            }
            _ => panic!("Expected Messages command"),
        }
    }

    #[test]
    fn parse_messages_rejects_legacy_id_flag() {
        let cli = Cli::try_parse_from(["tg", "messages", "--id", "123456789"]);
        assert!(cli.is_err());
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

    #[test]
    fn parse_auth_bot() {
        let cli = Cli::parse_from(["tg", "auth", "bot"]);
        match cli.command {
            Command::Auth(args) => {
                assert!(matches!(args.subcommand, Some(AuthSubcommand::Bot(_))));
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_auth_bot_with_token() {
        let cli = Cli::parse_from(["tg", "auth", "bot", "--token", "123:ABC"]);
        match cli.command {
            Command::Auth(args) => match args.subcommand {
                Some(AuthSubcommand::Bot(bot_args)) => {
                    assert_eq!(bot_args.token, Some("123:ABC".to_string()));
                }
                _ => panic!("Expected Auth Bot subcommand"),
            },
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_auth_status() {
        let cli = Cli::parse_from(["tg", "auth", "status"]);
        match cli.command {
            Command::Auth(args) => {
                assert!(matches!(args.subcommand, Some(AuthSubcommand::Status)));
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_auth_without_subcommand() {
        let cli = Cli::parse_from(["tg", "auth"]);
        match cli.command {
            Command::Auth(args) => {
                assert!(args.subcommand.is_none());
            }
            _ => panic!("Expected Auth command"),
        }
    }

    #[test]
    fn parse_send_with_as_and_to() {
        let cli = Cli::parse_from([
            "tg", "send", "--as", "@mybot", "--to", "@someone", "-m", "hello",
        ]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.send_as, Some("@mybot".to_string()));
                assert_eq!(args.to, Some("@someone".to_string()));
                assert_eq!(args.message.as_deref(), Some("hello"));
                assert_eq!(args.name, None);
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_with_to_numeric() {
        let cli = Cli::parse_from(["tg", "send", "--to", "123456", "-m", "hi"]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.to, Some("123456".to_string()));
                assert_eq!(args.send_as, None);
                assert_eq!(args.name, None);
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_with_to_name() {
        let cli = Cli::parse_from(["tg", "send", "--to", "John Doe", "-m", "hi"]);
        match cli.command {
            Command::Send(args) => {
                assert_eq!(args.to, Some("John Doe".to_string()));
                assert_eq!(args.name, None);
            }
            _ => panic!("Expected Send command"),
        }
    }

    #[test]
    fn parse_send_with_as_only_requires_to_or_id_or_name() {
        // --as alone without a recipient should fail
        let cli = Cli::try_parse_from(["tg", "send", "--as", "@mybot", "-m", "hi"]);
        assert!(cli.is_err());
    }

    #[test]
    fn parse_serve() {
        let cli = Cli::parse_from(["tg", "serve"]);
        assert!(matches!(cli.command, Command::Serve));
    }

    #[test]
    fn parse_sync_defaults() {
        let cli = Cli::parse_from(["tg", "sync"]);
        match cli.command {
            Command::Sync(args) => {
                assert_eq!(args.limit, 1000);
                assert!(args.reconcile_days.is_none());
            }
            _ => panic!("Expected Sync command"),
        }
    }

    #[test]
    fn parse_sync_with_reconcile_days() {
        let cli = Cli::parse_from(["tg", "sync", "--reconcile-days", "7"]);
        match cli.command {
            Command::Sync(args) => {
                assert_eq!(args.reconcile_days, Some(7));
            }
            _ => panic!("Expected Sync command"),
        }
    }

    #[test]
    fn parse_sync_with_limit() {
        let cli = Cli::parse_from(["tg", "sync", "--limit", "500"]);
        match cli.command {
            Command::Sync(args) => {
                assert_eq!(args.limit, 500);
            }
            _ => panic!("Expected Sync command"),
        }
    }
}
