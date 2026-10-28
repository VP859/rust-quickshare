//! `ferry`: command line, terminal UI and daemon of Ferry.

use anyhow::Result;
use clap::Parser;

use rqs_p2p::cli::args::{Cli, Commands};
use rqs_p2p::cli::nearby_cmd::{run_discover, run_pair};
use rqs_p2p::cli::peer_cmd::handle_peer_cmd;
use rqs_p2p::cli::settings_cmd::{run_language, run_name, run_network};
use rqs_p2p::cli::transfer_cmd::{run_receive, run_send, run_send_text, run_show_ticket};
use rqs_p2p::config::Config;
use rqs_p2p::daemon::run_daemon;
use rqs_p2p::telemetry::doctor::run_doctor;
use rqs_p2p::tui::run_tui;

/// Parses arguments, sets the language and logging, and runs the command.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let saved_language = Config::load().ok().and_then(|c| c.language);
    rqs_p2p::i18n::init(cli.lang.as_deref(), saved_language.as_deref());

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
        Commands::Text { text, to } => run_send_text(text, &to).await?,
        Commands::Receive { ticket, output_dir } => {
            run_receive(&ticket, output_dir.as_deref()).await?
        }
        Commands::Daemon { downloads_dir } => run_daemon(downloads_dir).await?,
        Commands::Tui => run_tui().await?,
        Commands::Ticket => run_show_ticket().await?,
        Commands::Discover { timeout } => run_discover(timeout).await?,
        Commands::Pair {
            target,
            auto_accept,
        } => run_pair(&target, auto_accept).await?,
        Commands::Name { name } => run_name(name.as_deref())?,
        Commands::Language { code } => run_language(code.as_deref())?,
        Commands::Network { mode } => run_network(mode.as_deref())?,
        Commands::Peer { command } => handle_peer_cmd(command).await?,
        Commands::Doctor => {
            run_doctor().await?;
        }
    }

    Ok(())
}
