use clap::Parser;
use std::path::Path;
use std::process::ExitCode;

use tg::cli::{Cli, Command};
use tg::client::TdLibClient;
use tg::commands::{
    chats, download, groups, mark_read, mark_unread, messages, search, send, unread,
};
use tg::credentials::{self, ApiCredentials};
use tg::error::{Result, TgError};
use tg::output::{
    DownloadStatus, OutputFormat, print_chats_table, print_contacts_table, print_error, print_list,
    print_messages_table, print_output, print_success,
};

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
    let is_auth = matches!(&command, Command::Auth(_));
    let data_dir = credentials::tg_data_dir();

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
            match format {
                OutputFormat::Json => print_list(format, &chats),
                OutputFormat::Plain => print_chats_table(&chats),
            }
        }

        Command::Groups(args) => {
            client.start().await?;
            let groups = groups::list_groups(client, args.limit).await?;
            match format {
                OutputFormat::Json => print_list(format, &groups),
                OutputFormat::Plain => print_chats_table(&groups),
            }
        }

        Command::Unread(args) => {
            client.start().await?;
            let unread = unread::list_unread(client, args.limit).await?;
            match format {
                OutputFormat::Json => print_list(format, &unread),
                OutputFormat::Plain => print_chats_table(&unread),
            }
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
            client.start().await?;
            let target = if let Some(id) = args.id {
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
            let target = if let Some(id) = args.chat {
                messages::ChatTarget::Id(id)
            } else {
                messages::ChatTarget::Name(args.name.unwrap())
            };

            let result =
                messages::get_messages(client, target, args.limit, args.since_utc.as_deref())
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
    }

    Ok(())
}
