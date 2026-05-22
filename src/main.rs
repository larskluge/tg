use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tg::bot_api;
use tg::cli::{Cli, Command};
use tg::client::TdLibClient;
use tg::commands::{
    auth_status, chats, download, groups, mark_read, mark_unread, messages, search, send, serve,
    sync, unread, whoami,
};
use tg::credentials::{self, ApiCredentials, BotEntry, CredentialsFile};
use tg::error::{Result, TgError};
use tg::output::{
    ChatInfo, ContactInfo, DownloadReport, DownloadStatus, MessageInfo, OutputFormat, SendResult,
    UserInfo, print_chats_table, print_contacts_table, print_error, print_list,
    print_messages_table, print_output, print_success,
};
use tg::resolve::{self, Recipient};
use tg::serve_client;
use tokio::net::UnixStream;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let format = OutputFormat::from_json_flag(cli.json);

    if let Err(e) = run(cli.command, format).await {
        print_error(&e.to_string());
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

async fn run(command: Command, format: OutputFormat) -> Result<()> {
    let data_dir = credentials::tg_data_dir();

    // Auth subcommands that don't need TDLib at all.
    if let Command::Auth(ref args) = command {
        match args.subcommand {
            Some(tg::cli::AuthSubcommand::Bot(ref bot_args)) => {
                return run_auth_bot(bot_args, &data_dir).await;
            }
            Some(tg::cli::AuthSubcommand::Status) => {
                let status = auth_status::build_auth_status(&data_dir)?;
                print_output(format, &status);
                return Ok(());
            }
            None => {
                // Interactive auth — refuse if `tg serve` is up, since both
                // want exclusive access to the TDLib on-disk database.
                if serve_client::is_running().await {
                    return Err(TgError::Other(
                        "tg auth: cannot run while `tg serve` is active. Stop the serve process and retry."
                            .to_string(),
                    ));
                }
            }
        }
    }

    // Bot sends use the HTTP API, never TDLib — bypass the proxy entirely.
    if let Command::Send(ref args) = command
        && args.send_as.is_some()
    {
        return run_bot_send(args, &data_dir, format).await;
    }

    // Serve owns the TDLib client and handles its own lifecycle.
    if matches!(command, Command::Serve) {
        let api_credentials = credentials::load_credentials_for_non_auth(&data_dir)?;
        let client = TdLibClient::new(api_credentials.api_id, api_credentials.api_hash)?;
        return serve::run(client).await;
    }

    // Try the warm serve socket first. If unreachable, fall through to the
    // in-process path so existing usage keeps working with no server up.
    let is_auth = matches!(&command, Command::Auth(a) if a.subcommand.is_none());
    if !is_auth && let Some(stream) = serve_client::try_connect().await {
        return route_via_serve(command, stream, format).await;
    }

    let api_credentials = if is_auth {
        match credentials::try_load_credentials_for_auth(&data_dir) {
            Some((creds, _)) => creds,
            None => credentials::prompt_credentials()?,
        }
    } else {
        credentials::load_credentials_for_non_auth(&data_dir)?
    };

    let pre_auth_storage = if is_auth {
        Some(check_auth_storage(&data_dir, &api_credentials))
    } else {
        None
    };
    let should_run_auth_flow = pre_auth_storage
        .map(|status| !status.all_stored())
        .unwrap_or(true);

    let mut client = TdLibClient::new(api_credentials.api_id, api_credentials.api_hash.clone())?;
    let result = if is_auth && !should_run_auth_flow {
        Ok(())
    } else {
        run_command(&mut client, command, format).await
    };

    // Always shut down the client gracefully
    client.shutdown().await;

    if is_auth {
        result?;

        if !should_run_auth_flow {
            println!("Already authenticated.");
            return Ok(());
        }

        // Keep credentials alongside session data after successful auth.
        credentials::save_credentials(&api_credentials, &data_dir)?;
        let post_auth_storage = check_auth_storage(&data_dir, &api_credentials);
        if !post_auth_storage.all_stored() {
            return Err(TgError::Other(
                "Authentication completed but failed to persist API credentials/session data"
                    .to_string(),
            ));
        }

        println!("Authenticated successfully!");
        return Ok(());
    }

    result
}

/// Forward a command to a running `tg serve` over an already-connected socket
/// and render the response locally.
async fn route_via_serve(command: Command, stream: UnixStream, format: OutputFormat) -> Result<()> {
    match command {
        Command::Whoami => {
            let info: UserInfo =
                serve_client::send_request(stream, "whoami", whoami::WhoamiRequest::default())
                    .await?;
            print_output(format, &info);
        }
        Command::Chats(args) => {
            let req = chats::ChatsRequest::from(args);
            let chats: Vec<ChatInfo> = serve_client::send_request(stream, "chats", req).await?;
            render_chat_list(format, &chats);
        }
        Command::Groups(args) => {
            let req = groups::GroupsRequest::from(args);
            let groups: Vec<ChatInfo> = serve_client::send_request(stream, "groups", req).await?;
            render_chat_list(format, &groups);
        }
        Command::Unread(args) => {
            let req = unread::UnreadRequest::from(args);
            let unread: Vec<ChatInfo> = serve_client::send_request(stream, "unread", req).await?;
            render_chat_list(format, &unread);
        }
        Command::Search(args) => {
            let req = search::SearchRequest::from(args);
            let contacts: Vec<ContactInfo> =
                serve_client::send_request(stream, "search", req).await?;
            match format {
                OutputFormat::Json => print_list(format, &contacts),
                OutputFormat::Plain => print_contacts_table(&contacts),
            }
        }
        Command::Messages(args) => {
            let req = messages::MessagesRequest::from(args);
            let msgs: Vec<MessageInfo> =
                serve_client::send_request(stream, "messages", req).await?;
            match format {
                OutputFormat::Json => print_list(format, &msgs),
                OutputFormat::Plain => print_messages_table(&msgs),
            }
        }
        Command::Send(args) => {
            // Bot sends are filtered out before route_via_serve.
            let req = send::SendRequest::from(args);
            let result: SendResult = serve_client::send_request(stream, "send", req).await?;
            print_output(format, &result);
        }
        Command::Download(args) => {
            let mut req = download::DownloadRequest::from(args);
            // Resolve output_dir relative to the CLIENT's CWD before sending —
            // the server may have a different working directory.
            req.output_dir = absolutize(&req.output_dir);
            let report: DownloadReport =
                serve_client::send_request(stream, "download", req).await?;
            print_output(format, &report);
            match report.status {
                DownloadStatus::NoDownloadableMedia => {
                    return Err(TgError::Other(
                        "Selected message has no downloadable media".to_string(),
                    ));
                }
                DownloadStatus::Failed => {
                    return Err(TgError::Other("One or more downloads failed".to_string()));
                }
                _ => {}
            }
        }
        Command::MarkRead(args) => {
            let req = mark_read::MarkReadRequest::from(args);
            let _: serde_json::Value = serve_client::send_request(stream, "mark_read", req).await?;
            print_success("Chat marked as read");
        }
        Command::MarkUnread(args) => {
            let req = mark_unread::MarkUnreadRequest::from(args);
            let _: serde_json::Value =
                serve_client::send_request(stream, "mark_unread", req).await?;
            print_success("Chat marked as unread");
        }
        Command::Sync(args) => {
            let hwm = read_sync_hwm_from_stdin()?;
            let req = sync::SyncRequest {
                hwm,
                limit: args.limit,
                reconcile_days: args.reconcile_days,
            };
            let results: HashMap<String, sync::SyncResult> =
                serve_client::send_request(stream, "sync", req).await?;
            println!("{}", serde_json::to_string_pretty(&results).unwrap());
            let has_errors = results
                .values()
                .any(|r| matches!(r, sync::SyncResult::Error { .. }));
            if has_errors {
                return Err(TgError::Other(
                    "One or more chats failed to sync".to_string(),
                ));
            }
        }
        Command::Serve | Command::Auth(_) => {
            unreachable!("Serve and Auth are routed before route_via_serve")
        }
    }
    Ok(())
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn render_chat_list(format: OutputFormat, chats: &[ChatInfo]) {
    match format {
        OutputFormat::Json => print_list(format, chats),
        OutputFormat::Plain => print_chats_table(chats),
    }
}

fn read_sync_hwm_from_stdin() -> Result<HashMap<String, i64>> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| TgError::Other(format!("Failed to read stdin: {e}")))?;
    let parsed: HashMap<String, i64> =
        serde_json::from_str(&buf).map_err(|e| TgError::Other(format!("Invalid JSON: {e}")))?;
    for k in parsed.keys() {
        if k.parse::<i64>().is_err() {
            return Err(TgError::Other(format!("Invalid chat ID: {k}")));
        }
    }
    Ok(parsed)
}

async fn run_auth_bot(args: &tg::cli::AuthBotArgs, data_dir: &std::path::Path) -> Result<()> {
    let token = match &args.token {
        Some(t) => t.clone(),
        None => {
            use std::io::{self, BufRead, Write};
            print!("Enter bot token (from @BotFather): ");
            io::stdout().flush().ok();
            io::stdin()
                .lock()
                .lines()
                .next()
                .ok_or_else(|| TgError::Other("Failed to read bot token".to_string()))?
                .map_err(|e| TgError::Other(e.to_string()))?
                .trim()
                .to_string()
        }
    };

    if token.is_empty() {
        return Err(TgError::Other("Bot token cannot be empty".to_string()));
    }

    let bot_user = bot_api::get_me(&token).await?;
    let username = bot_user
        .username
        .ok_or_else(|| TgError::Other("Bot has no username".to_string()))?;

    let bot_entry = BotEntry {
        id: bot_user.id,
        username: username.clone(),
        token,
    };

    let mut creds_file =
        credentials::load_credentials_file(data_dir).unwrap_or_else(|_| CredentialsFile {
            api_id: 0,
            api_hash: String::new(),
            user: None,
            bots: Vec::new(),
            known_contacts: Vec::new(),
        });

    creds_file.upsert_bot(bot_entry);
    credentials::save_credentials_file(&creds_file, data_dir)?;

    println!(
        "Bot @{} (ID: {}) authenticated successfully!",
        username, bot_user.id
    );
    Ok(())
}

async fn run_bot_send(
    args: &tg::cli::SendArgs,
    data_dir: &std::path::Path,
    format: OutputFormat,
) -> Result<()> {
    let send_as = args.send_as.as_ref().unwrap();
    let creds_file = credentials::load_credentials_file(data_dir)?;

    // Find the bot
    let bot = if let Ok(id) = send_as.parse::<i64>() {
        creds_file.find_bot_by_id(id)
    } else {
        creds_file.find_bot_by_username(send_as)
    }
    .ok_or_else(|| TgError::Other(format!("Bot {send_as} not found. Run `tg auth bot` first.")))?
    .clone();

    // Resolve the recipient
    let recipient = if let Some(ref to) = args.to {
        Recipient::To(to.clone())
    } else if let Some(id) = args.id {
        Recipient::Id(id)
    } else if let Some(ref group) = args.group {
        Recipient::Group(group.clone())
    } else {
        Recipient::Name(args.name.clone().unwrap())
    };
    let chat_id = resolve::resolve_recipient(recipient, &creds_file, data_dir).await?;

    let message_id = bot_api::send_message(&bot.token, chat_id, &args.message).await?;

    let result = SendResult {
        message_id,
        chat_id,
    };
    print_output(format, &result);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct AuthStorageStatus {
    api_id_stored: bool,
    api_hash_stored: bool,
    session_stored: bool,
}

impl AuthStorageStatus {
    fn all_stored(self) -> bool {
        self.api_id_stored && self.api_hash_stored && self.session_stored
    }
}

fn check_auth_storage(data_dir: &Path, expected: &ApiCredentials) -> AuthStorageStatus {
    let mut api_id_stored = false;
    let mut api_hash_stored = false;

    let creds_path = credentials::credentials_file_path(data_dir);
    if let Ok(raw) = std::fs::read_to_string(creds_path) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
            api_id_stored = json
                .get("api_id")
                .and_then(|v| v.as_i64())
                .map(|id| id == expected.api_id as i64)
                .unwrap_or(false);
            api_hash_stored = json
                .get("api_hash")
                .and_then(|v| v.as_str())
                .map(|hash| hash == expected.api_hash)
                .unwrap_or(false);
        }
    }

    let session_stored =
        dir_has_entries(&data_dir.join("db")) || dir_has_entries(&data_dir.join("files"));

    AuthStorageStatus {
        api_id_stored,
        api_hash_stored,
        session_stored,
    }
}

fn dir_has_entries(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => false,
    }
}

async fn run_command(
    client: &mut TdLibClient,
    command: Command,
    format: OutputFormat,
) -> Result<()> {
    match command {
        Command::Auth(_) => {
            tg::auth::authenticate(client).await?;
        }

        Command::Chats(args) => {
            client.start().await?;
            let chats = chats::list_chats(client, args.limit).await?;
            render_chat_list(format, &chats);
        }

        Command::Groups(args) => {
            client.start().await?;
            let groups = groups::list_groups(client, args.limit).await?;
            render_chat_list(format, &groups);
        }

        Command::Unread(args) => {
            client.start().await?;
            let unread = unread::list_unread(client, args.limit).await?;
            render_chat_list(format, &unread);
        }

        Command::Search(args) => {
            client.start().await?;
            let contacts = search::search_contacts(client, &args.query).await?;
            match format {
                OutputFormat::Json => print_list(format, &contacts),
                OutputFormat::Plain => print_contacts_table(&contacts),
            }
        }

        Command::Send(args) => {
            // --as bot sends are handled before run_command, so this is always user send.
            client.start().await?;
            let target = if let Some(ref to) = args.to {
                if let Ok(id) = to.parse::<i64>() {
                    send::SendTarget::Id(id)
                } else {
                    let name = to.strip_prefix('@').unwrap_or(to);
                    send::SendTarget::Name(name.to_string())
                }
            } else if let Some(id) = args.id {
                send::SendTarget::Id(id)
            } else if let Some(group) = args.group {
                send::SendTarget::Group(group)
            } else {
                send::SendTarget::Name(args.name.unwrap())
            };

            let result = send::send_message(client, target, &args.message).await?;
            print_output(format, &result);
        }

        Command::Messages(args) => {
            client.start().await?;
            if args.since_utc.is_some() {
                client.wait_for_sync().await;
            }
            let target = if let Some(id) = args.chat {
                messages::ChatTarget::Id(id)
            } else {
                messages::ChatTarget::Name(args.name.unwrap())
            };

            let result = messages::get_messages(
                client,
                target,
                args.limit,
                args.since_utc.as_deref(),
                args.oldest_first,
            )
            .await?;
            if result.messages.is_empty()
                && let Some(ref since) = args.since_utc
            {
                eprintln!("No new messages in chat {} since {}", result.chat_id, since);
            }
            match format {
                OutputFormat::Json => print_list(format, &result.messages),
                OutputFormat::Plain => print_messages_table(&result.messages),
            }
        }

        Command::Download(args) => {
            client.start().await?;
            let report = download::download_message_media(
                client,
                args.chat,
                args.message,
                args.output_dir,
                args.priority,
            )
            .await?;
            print_output(format, &report);

            match report.status {
                DownloadStatus::NoDownloadableMedia => {
                    return Err(TgError::Other(
                        "Selected message has no downloadable media".to_string(),
                    ));
                }
                DownloadStatus::Failed => {
                    return Err(TgError::Other("One or more downloads failed".to_string()));
                }
                _ => {}
            }
        }

        Command::Whoami => {
            client.start().await?;
            let info = whoami::whoami(client).await?;
            print_output(format, &info);
        }

        Command::MarkRead(args) => {
            client.start().await?;
            let target = if let Some(id) = args.id {
                mark_read::ChatTarget::Id(id)
            } else {
                mark_read::ChatTarget::Name(args.name.unwrap())
            };

            mark_read::mark_as_read(client, target).await?;
            print_success("Chat marked as read");
        }

        Command::MarkUnread(args) => {
            client.start().await?;
            let target = if let Some(id) = args.id {
                mark_unread::ChatTarget::Id(id)
            } else {
                mark_unread::ChatTarget::Name(args.name.unwrap())
            };

            mark_unread::mark_as_unread(client, target).await?;
            print_success("Chat marked as unread");
        }

        Command::Sync(args) => {
            client.start().await?;
            client.wait_for_sync().await;

            let input = {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| TgError::Other(format!("Failed to read stdin: {e}")))?;
                buf
            };

            let hwm_map = sync::parse_hwm_input(&input).map_err(TgError::Other)?;
            let results = sync::sync_chats(client, hwm_map, args.limit, args.reconcile_days).await;

            let has_errors = results
                .values()
                .any(|r| matches!(r, sync::SyncResult::Error { .. }));

            // Always output JSON (machine-only command)
            println!("{}", serde_json::to_string_pretty(&results).unwrap());

            if has_errors {
                return Err(TgError::Other(
                    "One or more chats failed to sync".to_string(),
                ));
            }
        }

        Command::Serve => {
            // Serve is handled at the top of `run()` because it owns the
            // TDLib client and its own shutdown lifecycle.
            unreachable!("Serve is handled at the top of run()");
        }
    }

    Ok(())
}
