//! `ferry peer`: management of trusted devices.

use anyhow::{Context, Result};
use rust_i18n::t;

use crate::cli::args::PeerCommands;
use crate::cli::nearby_cmd::run_peer_add;
use ferry::config::Config;
use ferry::pairing::safety_number;
use iroh::EndpointId;

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
        PeerCommands::Verify {
            target,
            confirm,
            unconfirm,
        } => verify(&target, confirm, unconfirm),
        PeerCommands::Remove { target } => remove(&target).await,
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
                // A check mark: the safety number was compared (`ferry peer verify`).
                alias = format!(
                    "{:<20}",
                    if info.verified {
                        format!("{} ✓", info.alias)
                    } else {
                        info.alias.clone()
                    }
                ),
                id = node_id,
                auto = info.auto_accept,
                added = info.added_at.format("%Y-%m-%d %H:%M")
            )
        );
    }
    println!("==========================================================\n");
    Ok(())
}

/// `ferry peer verify`: shows the safety number shared with a device and
/// records the result of comparing it.
fn verify(target: &str, confirm: bool, unconfirm: bool) -> Result<()> {
    let mut config = Config::load()?;
    let key = config
        .find_key(target)
        .with_context(|| t!("peer.not_found", target = target))?;
    let peer: EndpointId = key
        .parse()
        .with_context(|| t!("config.corrupted_node_id"))?;
    let me = ferry::node::load_or_generate_default_key()?.public();
    let alias = config.peers[&key].alias.clone();

    println!("{}", t!("peer.verify_title", alias = &alias));
    println!("\n{}\n", wrap_groups(&safety_number(&me, &peer)));
    println!("{}", t!("peer.verify_how"));
    if confirm || unconfirm {
        config.set_verified(&key, confirm);
        config.save()?;
        println!(
            "\n{}",
            if confirm {
                t!("peer.verify_done", alias = &alias)
            } else {
                t!("peer.verify_removed", alias = &alias)
            }
        );
    } else {
        let state = if config.peers[&key].verified {
            t!("peer.verify_state_yes")
        } else {
            t!("peer.verify_state_no")
        };
        println!("\n{state}");
    }
    Ok(())
}

/// A safety number in three lines of four groups, easier to read out.
fn wrap_groups(number: &str) -> String {
    number
        .split(' ')
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|line| format!("  {}", line.join("  ")))
        .collect::<Vec<_>>()
        .join("\n")
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
///
/// The device is told about it when it can be reached, so it stops listing
/// this one as paired.
async fn remove(target: &str) -> Result<()> {
    let mut config = Config::load()?;
    let address = config.resolve_target(target).ok();
    let key = config.find_key(target);
    if !config.remove_peer(target) {
        println!("{}", t!("peer.not_found", target = target));
        return Ok(());
    }
    if let Some(key) = &key {
        config.queue_unpair(key);
    }
    config.save()?;
    println!("{}", t!("peer.removed", target = target));
    if let Some(address) = address {
        let told = async {
            let engine = ferry::node::NodeEngine::with_persistent_identity().await?;
            let result = ferry::transfer::service::send_unpair(&engine, address).await;
            engine.close().await;
            result
        }
        .await;
        match told {
            Ok(()) => {
                println!("{}", t!("peer.removed_told"));
                if let Some(key) = &key {
                    forget_unpair(key);
                }
            }
            Err(_) => println!("{}", t!("peer.removed_not_told")),
        }
    }
    Ok(())
}

/// Stops waiting to tell the removed device `node_id` (it was told).
pub fn forget_unpair(node_id: &str) {
    if let Ok(mut config) = Config::load()
        && config.pending_unpair.remove(node_id).is_some()
    {
        let _ = config.save();
    }
}

/// Tells the removed devices among `nearby` that they were removed, when they
/// are waiting for it (`config.pending_unpair`), and forgets those told.
pub async fn deliver_pending_unpairs(
    engine: &ferry::node::NodeEngine,
    nearby: &[ferry::discovery::NearbyDevice],
) {
    let pending = Config::load()
        .map(|c| c.pending_unpair_ids())
        .unwrap_or_default();
    if pending.is_empty() {
        return;
    }
    for id in ferry::transfer::service::deliver_unpairs(engine, nearby, &pending).await {
        forget_unpair(&id);
    }
}
