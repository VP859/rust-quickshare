use anyhow::Result;
use clap::Parser;

use rqs_p2p::cli::args::{Cli, Commands};
use rqs_p2p::cli::peer_cmd::handle_peer_cmd;
use rqs_p2p::cli::transfer_cmd::{run_receive, run_send, run_show_ticket};
use rqs_p2p::daemon::run_daemon;
use rqs_p2p::telemetry::doctor::run_doctor;
use rqs_p2p::tui::run_tui;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // TUI rysuje po całym terminalu – logi tylko z RUST_LOG, aby nie psuły widoku.
    let default_filter = if matches!(cli.command, Commands::Tui) {
        "off"
    } else {
        "info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
        )
        .init();

    match cli.command {
        Commands::Send { files, to } => run_send(&files, to.as_deref()).await?,
        Commands::Receive { ticket, output_dir } => {
            run_receive(&ticket, output_dir.as_deref()).await?
        }
        Commands::Daemon { downloads_dir } => run_daemon(downloads_dir).await?,
        Commands::Tui => run_tui().await?,
        Commands::Ticket => run_show_ticket().await?,
        Commands::Peer { command } => handle_peer_cmd(command)?,
        Commands::Doctor => {
            run_doctor().await?;
        }
    }

    Ok(())
}
