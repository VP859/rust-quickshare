//! `ferry peer`: management of trusted devices.

use anyhow::{Context, Result};
use rust_i18n::t;

use crate::cli::args::PeerCommands;
use crate::cli::nearby_cmd::run_peer_add;
use crate::config::Config;

/// Runs a `ferry peer` subcommand.
///
/// # Errors
///
/// Fails when the configuration cannot be loaded or saved, or the device is unknown.
pub async fn handle_peer_cmd(cmd: PeerCommands) -> Result<()> {
    match cmd {
        PeerCommands::Add {
            ticket,
            alias,
            auto_accept,
            no_pair,
        } => run_peer_add(&ticket, &alias, auto_accept, no_pair).await,
        PeerCommands::List => list(),
        PeerCommands::Edit {
            target,
            alias,
            auto_accept,
        } => edit(&target, alias, auto_accept),
        PeerCommands::Remove { target } => remove(&target),
    }
}

/// `ferry peer list`: prints trusted devices sorted by alias.
fn list() -> Result<()> {
    let config = Config::load()?;
    if config.peers.is_empty() {
        println!("{}", t!("peer.none", path = Config::file_path()?.display()));
        return Ok(());
    }

    println!(
        "\n=================== {} ===================",
        t!("peer.title")
    );
    let mut peers: Vec<_> = config.peers.iter().collect();
    peers.sort_by_key(|(_, p)| p.alias.to_lowercase());
    for (node_id, info) in peers {
        println!(
            " • {}",
            t!(
                "peer.row",
                alias = format!("{:<20}", info.alias),
                id = node_id,
                auto = info.auto_accept,
                added = info.added_at.format("%Y-%m-%d %H:%M")
            )
        );
    }
    println!("==========================================================\n");
    Ok(())
}

/// `ferry peer edit`: changes the alias and/or Auto-Accept of a device.
fn edit(target: &str, alias: Option<String>, auto_accept: Option<bool>) -> Result<()> {
    let mut config = Config::load()?;
    let key = config
        .find_key(target)
        .with_context(|| t!("peer.not_found", target = target))?;
    if let Some(peer) = config.peers.get_mut(&key) {
        if let Some(alias) = alias {
            peer.alias = alias;
        }
        if let Some(auto_accept) = auto_accept {
            peer.auto_accept = auto_accept;
        }
        println!(
            "{}",
            t!("peer.updated", alias = &peer.alias, auto = peer.auto_accept)
        );
    }
    config.save()
}

/// `ferry peer remove`: removes a device by alias or Node ID.
fn remove(target: &str) -> Result<()> {
    let mut config = Config::load()?;
    if config.remove_peer(target) {
        config.save()?;
        println!("{}", t!("peer.removed", target = target));
    } else {
        println!("{}", t!("peer.not_found", target = target));
    }
    Ok(())
}
