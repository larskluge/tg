use clap::Parser;
use std::env;
use std::process::ExitCode;

use tg::cli::{Cli, Command};
use tg::client::TdLibClient;
use tg::commands::{chats, groups, mark_read, mark_unread, messages, search, send, unread};
use tg::error::{Result, TgError};
use tg::output::{
    print_chats_table, print_contacts_table, print_error, print_list, print_output, print_success,
    OutputFormat,
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
    let api_id: i32 = env::var("TG_API_ID")
        .map_err(|_| TgError::EnvVarMissing("TG_API_ID".to_string()))?
        .parse()
        .map_err(|_| TgError::Other("TG_API_ID must be a number".to_string()))?;

    let api_hash = env::var("TG_API_HASH")
        .map_err(|_| TgError::EnvVarMissing("TG_API_HASH".to_string()))?;

    let mut client = TdLibClient::new(api_id, api_hash)?;

    let result = run_command(&mut client, command, format).await;

    // Always shut down the client gracefully
    client.shutdown().await;

    result
}

async fn run_command(client: &mut TdLibClient, command: Command, format: OutputFormat) -> Result<()> {
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
            let target = if let Some(id) = args.id {
                messages::ChatTarget::Id(id)
            } else {
                messages::ChatTarget::Name(args.name.unwrap())
            };

            let msgs = messages::get_messages(client, target, args.limit).await?;
            print_list(format, &msgs);
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
