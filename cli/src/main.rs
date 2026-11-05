//! `ferry`: command line, terminal UI and daemon of Ferry, built on the
//! `ferry` library.

rust_i18n::i18n!("../locales", fallback = "en");

mod cli;
mod daemon;
mod doctor;
mod tui;

use anyhow::Result;
use clap::Parser;

use crate::cli::args::{Cli, Commands};
use crate::cli::history_cmd::run_history;
use crate::cli::identity_cmd::run_identity;
use crate::cli::integrate_cmd::run_integrate;
use crate::cli::nearby_cmd::{run_discover, run_pair};
use crate::cli::peer_cmd::handle_peer_cmd;
use crate::cli::settings_cmd::{run_config, run_language, run_name, run_network};
use crate::cli::transfer_cmd::{run_receive, run_send, run_send_text, run_show_ticket};
use crate::cli::web_cmd::run_web;
use crate::daemon::run_daemon;
use crate::doctor::run_doctor;
use crate::tui::run_tui;
use ferry::config::Config;

/// Parses arguments, sets the language and logging, and runs the command.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = Config::load().unwrap_or_default();
    ferry::i18n::init(cli.lang.as_deref(), config.language.as_deref());
    // Every command that transfers files records them and keeps the speed limit.
    ferry::history::enable_from_config(&config);
    ferry::transfer::control::set_speed_limit(config.speed_limit);

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
        Commands::Send { files, to } => run_send(&files, &to).await?,
        Commands::Integrate { remove } => run_integrate(remove)?,
        Commands::History {
            query,
            limit,
            kind,
            clear,
        } => run_history(query.as_deref(), limit, kind, clear)?,
        Commands::Web {
            files,
            no_receive,
            minutes,
        } => run_web(files, no_receive, minutes).await?,
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
        Commands::Network { mode, relays } => run_network(mode.as_deref(), &relays)?,
        Commands::Peer { command } => handle_peer_cmd(command).await?,
        Commands::Config { command } => run_config(command)?,
        Commands::Identity { command } => run_identity(command)?,
        Commands::Doctor => {
            // A failed check gives exit code 1, e.g. for scripts and support.
            if !run_doctor().await?.healthy() {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
