//! `ferry discover`, `ferry pair` and adding devices with a pairing request.

use anyhow::{Context, Result};
use rust_i18n::t;
use std::time::Duration;

use ferry::config::Config;
use ferry::discovery::{Discovery, DiscoveryHealth, DiscoveryOptions, NearbyDevice};
use ferry::node::{NodeEngine, NodeTicket};
use ferry::pairing::{PairOutcome, request_pairing};

/// How long `ferry pair` looks for the named device on the local network.
const PAIR_LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// How long `ferry peer add` looks for a fresher LAN address of the device.
const NEARBY_LOOKUP: Duration = Duration::from_millis(1500);

/// Prints the table of nearby devices, or a hint why none were found.
fn print_devices(devices: &[NearbyDevice], config: &Config, health: &DiscoveryHealth) {
    if devices.is_empty() {
        println!("{}", t!("nearby.none"));
        if let Some(hint) = health.hint() {
            println!("{}", t!("nearby.hint", hint = hint));
        }
        return;
    }
    println!(
        "\n {:<3} {:<28} {:<9} {:<18} {}",
        "#",
        t!("nearby.col_name"),
        t!("nearby.col_system"),
        "NODE ID",
        t!("nearby.col_status")
    );
    for (i, d) in devices.iter().enumerate() {
        let status = match config.find_peer(&d.node_id) {
            Some(p) => t!("nearby.paired_as", alias = &p.alias),
            None => t!("nearby.pair_hint"),
        };
        println!(
            " {:<3} {:<28} {:<9} {:<18} {status}",
            i + 1,
            d.name,
            d.platform,
            &d.node_id[..16]
        );
    }
    println!();
}

/// `ferry discover`: lists devices found on the local network within `timeout` seconds.
///
/// # Errors
///
/// Fails when the node or the discovery socket cannot start.
pub async fn run_discover(timeout: u64) -> Result<()> {
    let config = Config::load()?;
    let engine = NodeEngine::with_persistent_identity().await?;
    let discovery = Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::from_config(&config),
    )?;
    println!(
        "{}",
        t!(
            "nearby.searching",
            seconds = timeout,
            name = config.device_name()
        )
    );
    discovery.probe();
    tokio::time::sleep(Duration::from_secs(timeout)).await;
    print_devices(&discovery.devices(), &config, &discovery.health());
    drop(discovery);
    engine.close().await;
    Ok(())
}

/// `ferry pair <name|Node ID|TICKET>`: pairs with a device and stores it as trusted.
///
/// Outside the local network a TICKET or Node ID can still be used.
///
/// # Errors
///
/// Fails when the device cannot be found or reached, or declines the request.
pub async fn run_pair(target: &str, auto_accept: bool) -> Result<()> {
    let mut config = Config::load()?;
    let my_name = config.device_name();
    let engine = NodeEngine::with_persistent_identity().await?;
    let discovery = Discovery::start(
        engine.clone(),
        &my_name,
        DiscoveryOptions::from_config(&config),
    )?;

    println!("{}", t!("nearby.looking_for", target = target));
    let (addr, label) = match discovery.wait_for(target, PAIR_LOOKUP_TIMEOUT).await {
        Some(d) => (d.addr, d.name),
        None => (
            NodeTicket::parse_target(target)
                .with_context(|| t!("nearby.not_found", target = target))?,
            target.to_string(),
        ),
    };
    println!("\n{}", t!("pair.pairing_with", name = &label));
    let result = request_pairing(&engine, addr.clone(), &my_name, |code| {
        println!("{}", t!("pair.code", code = code));
        println!("{}\n", t!("pair.confirm_other"));
    })
    .await;
    drop(discovery);
    engine.close().await;

    match result? {
        PairOutcome::Accepted { name, addr: remote } => {
            config.add_paired(addr.id, &name, remote.or(Some(addr.clone())), auto_accept)?;
            config.save()?;
            let alias = config
                .find_peer(&addr.id.to_string())
                .map(|p| p.alias.clone())
                .unwrap_or(name);
            println!("{}", t!("pair.done", alias = &alias, auto = auto_accept));
            println!("{}", t!("pair.send_hint", alias = &alias));
            Ok(())
        }
        PairOutcome::Rejected => anyhow::bail!(t!("pair.rejected")),
    }
}

/// `ferry pairing [--minutes N] [--close]`: opens pairing for unknown devices
/// for a while, or closes it. Needed only with `paired-only`; a running daemon
/// or TUI reads the configuration for every request.
///
/// # Errors
///
/// Fails when the configuration cannot be loaded or saved.
pub fn run_pairing(minutes: u32, close: bool) -> Result<()> {
    let mut config = Config::load()?;
    if close {
        config.close_pairing();
        config.save()?;
        println!("{}", t!("pair.closed"));
        return Ok(());
    }
    config.open_pairing(i64::from(minutes));
    config.save()?;
    let minutes = config
        .pairing_until
        .map_or(0, |until| (until - chrono::Utc::now()).num_minutes() + 1);
    println!("{}", t!("pair.opened", minutes = minutes));
    if !config.paired_only {
        println!("{}", t!("pair.not_needed"));
    }
    Ok(())
}

/// `ferry peer add <TICKET|Node ID>`: stores the device and sends it a pairing
/// request, so that after confirmation the other side trusts this device too.
///
/// A failed or declined pairing is reported but not treated as an error: the
/// device stays trusted on this side.
///
/// # Errors
///
/// Fails when the input is invalid or the configuration cannot be saved.
pub async fn run_peer_add(
    input: &str,
    alias: &str,
    auto_accept: bool,
    no_pair: bool,
) -> Result<()> {
    let mut config = Config::load()?;
    let node_id = config.add_from_input(input, alias, auto_accept)?;
    config.save()?;
    let saved_alias = config
        .find_peer(&node_id)
        .map(|p| p.alias.clone())
        .unwrap_or_default();
    println!(
        "{}",
        t!(
            "peer.added",
            alias = &saved_alias,
            id = &node_id,
            auto = auto_accept
        )
    );
    if no_pair {
        return Ok(());
    }

    let my_name = config.device_name();
    let engine = NodeEngine::with_persistent_identity().await?;
    let mut addr = config.resolve_target(&node_id)?;
    if let Ok(discovery) = Discovery::start(
        engine.clone(),
        &my_name,
        DiscoveryOptions::from_config(&config),
    ) && let Some(nearby) = discovery.wait_for(&node_id, NEARBY_LOOKUP).await
    {
        addr = nearby.addr;
    }
    let result = request_pairing(&engine, addr.clone(), &my_name, |code| {
        println!("{}", t!("pair.request_sent", code = code));
    })
    .await;
    engine.close().await;
    match result {
        Ok(PairOutcome::Accepted { addr: remote, .. }) => {
            config.add_paired(addr.id, &saved_alias, remote, auto_accept)?;
            config.save()?;
            println!("{}", t!("pair.mutual"));
        }
        Ok(PairOutcome::Rejected) => println!("{}", t!("pair.one_sided_rejected")),
        Err(e) => println!("{}", t!("pair.one_sided_failed", error = format!("{e:#}"))),
    }
    Ok(())
}
