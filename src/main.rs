use clap::Parser;
use std::process::ExitCode;

use tg::cli::{Cli, Command};
use tg::client::TdLibClient;
use tg::commands::{
    chats, download, groups, mark_read, mark_unread, messages, search, send, unread,
};
use tg::credentials::{self, CredentialSource};
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

    let (api_credentials, credential_source) = if is_auth {
        credentials::load_credentials_for_auth(&data_dir)?
    } else {
        (
            credentials::load_credentials_for_non_auth(&data_dir)?,
            CredentialSource::Stored,
        )
    };

    let mut client = TdLibClient::new(api_credentials.api_id, api_credentials.api_hash.clone())?;

    let result = run_command(&mut client, command, format).await;

    // Always shut down the client gracefully
    client.shutdown().await;

    if is_auth && result.is_ok() && credential_source == CredentialSource::Env {
        credentials::save_credentials(&api_credentials, &data_dir)?;
    }

    result
}

async fn run_command(
    client: &mut TdLibClient,
    command: Command,
    format: OutputFormat,
) -> Result<()> {
    match command {
        Command::Auth(args) => {
            tg::auth::authenticate(client, args.phone.as_deref()).await?;
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

            let msgs = messages::get_messages(client, target, args.limit).await?;
            match format {
                OutputFormat::Json => print_list(format, &msgs),
                OutputFormat::Plain => print_messages_table(&msgs),
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
