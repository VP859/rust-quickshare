use anyhow::{Context, Result};
use std::time::Duration;

use crate::config::Config;
use crate::discovery::{Discovery, DiscoveryOptions, NearbyDevice};
use crate::node::{NodeEngine, NodeTicket};
use crate::pairing::{PairOutcome, clean_device_name, pairing_code, request_pairing};

/// Czas oczekiwania na urządzenie podane w `rqs pair`.
const PAIR_LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);

fn print_devices(devices: &[NearbyDevice], config: &Config) {
    if devices.is_empty() {
        println!("Nie znaleziono urządzeń RQS w sieci lokalnej.");
        println!(
            "Sprawdź, czy druga aplikacja działa w tej samej sieci i czy zapora przepuszcza UDP {}.",
            crate::discovery::DISCOVERY_PORT
        );
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
    print_devices(&discovery.devices(), &config);
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
