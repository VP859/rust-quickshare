use anyhow::{Context, Result};
use std::time::Duration;

use crate::config::Config;
use crate::discovery::{Discovery, DiscoveryHealth, DiscoveryOptions, NearbyDevice};
use crate::node::{NodeEngine, NodeTicket};
use crate::pairing::{PairOutcome, clean_device_name, pairing_code, request_pairing};

/// Czas oczekiwania na urządzenie podane w `rqs pair`.
const PAIR_LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);

fn print_devices(devices: &[NearbyDevice], config: &Config, health: &DiscoveryHealth) {
    if devices.is_empty() {
        println!("Nie znaleziono urządzeń RQS w sieci lokalnej.");
        if let Some(hint) = health.hint() {
            println!("Wskazówka: {hint}");
        }
        return;
    }
    println!(
        "\n {:<3} {:<28} {:<9} {:<18} STATUS",
        "#", "NAZWA", "SYSTEM", "NODE ID"
    );
    for (i, d) in devices.iter().enumerate() {
        let status = match config.find_peer(&d.node_id) {
            Some(p) => format!("sparowane jako '{}'", p.alias),
            None => "rqs pair <nazwa>".to_string(),
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

/// `rqs discover`
pub async fn run_discover(timeout: u64) -> Result<()> {
    let config = Config::load()?;
    let engine = NodeEngine::with_persistent_identity().await?;
    let discovery = Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::default(),
    )?;
    println!(
        "Szukanie urządzeń przez {timeout} s (to urządzenie: '{}')...",
        config.device_name()
    );
    discovery.probe();
    tokio::time::sleep(Duration::from_secs(timeout)).await;
    print_devices(&discovery.devices(), &config, &discovery.health());
    drop(discovery);
    engine.close().await;
    Ok(())
}

/// `rqs pair <nazwa|Node ID|TICKET>`
pub async fn run_pair(target: &str, auto_accept: bool) -> Result<()> {
    let mut config = Config::load()?;
    let my_name = config.device_name();
    let engine = NodeEngine::with_persistent_identity().await?;
    let discovery = Discovery::start(engine.clone(), &my_name, DiscoveryOptions::default())?;

    println!("Szukanie '{target}' w sieci lokalnej...");
    let (addr, label) = match discovery.wait_for(target, PAIR_LOOKUP_TIMEOUT).await {
        Some(d) => (d.addr, d.name),
        // Poza siecią lokalną nadal można parować po TICKECIE lub Node ID.
        None => (
            NodeTicket::parse_target(target).with_context(|| {
                format!("Nie znaleziono urządzenia '{target}' w sieci lokalnej")
            })?,
            target.to_string(),
        ),
    };
    let code = pairing_code(&engine.node_id(), &addr.id);
    println!("\nParowanie z '{label}'.");
    println!("Kod weryfikacyjny: {code}");
    println!("Potwierdź na drugim urządzeniu i upewnij się, że widzi ten sam kod...\n");

    let result = request_pairing(&engine, addr.clone(), &my_name).await;
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
            println!("[SUKCES] Sparowano z '{alias}' (Auto-Accept: {auto_accept}).");
            println!("Wysyłka: rqs send <plik> --to \"{alias}\"");
            Ok(())
        }
        PairOutcome::Rejected => anyhow::bail!("Drugie urządzenie odrzuciło parowanie"),
    }
}

/// `rqs peer add <TICKET|Node ID>` – zapis lokalny + prośba o sparowanie, aby druga strona
/// (po potwierdzeniu) również dodała to urządzenie do zaufanych.
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
        "[SUKCES] Dodano urządzenie '{saved_alias}' ({node_id}) (Auto-Accept: {auto_accept})."
    );
    if no_pair {
        return Ok(());
    }

    let my_name = config.device_name();
    let engine = NodeEngine::with_persistent_identity().await?;
    let mut addr = config.resolve_target(&node_id)?;
    if let Ok(discovery) = Discovery::start(engine.clone(), &my_name, DiscoveryOptions::default())
        && let Some(nearby) = discovery
            .wait_for(&node_id, Duration::from_millis(1500))
            .await
    {
        addr = nearby.addr;
    }
    println!(
        "Prośba o sparowanie wysłana – potwierdź na drugim urządzeniu (kod: {})...",
        pairing_code(&engine.node_id(), &addr.id)
    );
    let result = request_pairing(&engine, addr.clone(), &my_name).await;
    engine.close().await;
    match result {
        Ok(PairOutcome::Accepted { addr: remote, .. }) => {
            config.add_paired(addr.id, &saved_alias, remote, auto_accept)?;
            config.save()?;
            println!("[SUKCES] Sparowano obustronnie – drugie urządzenie też ufa temu urządzeniu.");
        }
        Ok(PairOutcome::Rejected) => {
            println!("[INFO] Drugie urządzenie odrzuciło parowanie – dodano tylko po tej stronie.")
        }
        Err(e) => println!("[INFO] Nie udało się sparować ({e:#}) – dodano tylko po tej stronie."),
    }
    Ok(())
}

/// `rqs name [nowa nazwa]`
pub fn run_name(name: Option<&str>) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(name) = name {
        let clean = clean_device_name(name);
        if clean.is_empty() {
            anyhow::bail!("Nazwa nie może być pusta");
        }
        config.device_name = Some(clean);
        config.save()?;
    }
    println!("Nazwa urządzenia: {}", config.device_name());
    Ok(())
}
