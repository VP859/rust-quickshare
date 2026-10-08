use anyhow::Result;
use clap::Parser;

use rust_quickshare::cli::args::{Cli, Commands};
use rust_quickshare::cli::peer_cmd::handle_peer_cmd;
use rust_quickshare::daemon::run_daemon;
use rust_quickshare::transfer::receiver::run_receiver;
use rust_quickshare::transfer::sender::run_sender;
use rust_quickshare::tui::run_tui;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Send { file_path } => {
            run_sender(&file_path).await?;
        }
        Commands::Receive {
            ticket,
            output_path,
        } => {
            run_receiver(&ticket, output_path.as_deref()).await?;
        }
        Commands::Daemon { downloads_dir } => {
            run_daemon(downloads_dir).await?;
        }
        Commands::Tui => {
            run_tui().await?;
        }
        Commands::Peer { command } => {
            handle_peer_cmd(command)?;
        }
    }

    Ok(())
}
