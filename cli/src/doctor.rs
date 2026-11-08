//! `ferry doctor`: diagnostics of the network environment and configuration.

use anyhow::Result;
#[cfg(not(target_os = "android"))]
use network_interface::NetworkInterfaceConfig;
use rust_i18n::t;
use std::net::UdpSocket;
#[cfg(not(target_os = "android"))]
use tracing::info;

/// Summary of the checks performed by [`run_doctor`].
pub struct DoctorReport {
    /// A UDP socket can be opened.
    pub udp_socket_ok: bool,
    /// `config.toml` is missing or parses correctly.
    pub config_valid: bool,
}

impl DoctorReport {
    /// No check found a problem that prevents Ferry from working.
    pub fn healthy(&self) -> bool {
        self.udp_socket_ok && self.config_valid
    }
}

/// Runs all checks, prints a human readable report and returns its summary.
///
/// # Errors
///
/// Currently infallible; individual check failures are part of the report.
pub async fn run_doctor() -> Result<DoctorReport> {
    println!("\n=== {} ===", t!("doctor.title"));

    let udp_ok = match UdpSocket::bind("0.0.0.0:0") {
        Ok(_) => {
            println!("[✓] {}", t!("doctor.udp_ok"));
            true
        }
        Err(e) => {
            println!("[✗] {}", t!("doctor.udp_failed", error = e));
            false
        }
    };

    let config = ferry::config::Config::load().unwrap_or_default();
    println!(
        "[i] {}",
        t!("doctor.network_mode", mode = config.network_mode)
    );
    if config.network_mode == ferry::node::NetworkMode::Relay {
        match ferry::node::Network::from_config(&config) {
            Ok(network) => {
                for relay in &network.relays {
                    println!("[i] {}", t!("doctor.relay", url = relay));
                }
            }
            Err(e) => println!("[✗] {e:#}"),
        }
    }

    let vpn_detected = check_vpn_interfaces();
    if vpn_detected {
        println!("[i] {}", t!("doctor.vpn_found"));
    } else {
        println!("[✓] {}", t!("doctor.vpn_none"));
    }

    check_discovery().await;

    let config_ok = match ferry::config::Config::load() {
        Ok(_) => {
            println!("[✓] {}\n", t!("doctor.config_ok"));
            true
        }
        Err(e) => {
            println!(
                "[✗] {}\n",
                t!("doctor.config_failed", error = format!("{e:#}"))
            );
            false
        }
    };

    Ok(DoctorReport {
        udp_socket_ok: udp_ok,
        config_valid: config_ok,
    })
}

/// Runs local network discovery for a few seconds and prints what it found.
async fn check_discovery() {
    use ferry::discovery::{DISCOVERY_PORT, Discovery, DiscoveryOptions};
    let config = ferry::config::Config::load().unwrap_or_default();
    let engine = match ferry::node::NodeEngine::with_persistent_identity().await {
        Ok(e) => e,
        Err(e) => {
            println!(
                "[✗] {}",
                t!("doctor.discovery_failed", error = format!("{e:#}"))
            );
            return;
        }
    };
    match Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::from_config(&config),
    ) {
        Ok(discovery) => {
            discovery.probe();
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            let health = discovery.health();
            let devices = discovery.devices();
            let lan = if health.lan_send_ok {
                t!("doctor.ok")
            } else {
                t!("doctor.blocked")
            };
            println!(
                "[{}] {}",
                if health.hint().is_none() { "✓" } else { "!" },
                t!(
                    "doctor.discovery",
                    port = DISCOVERY_PORT,
                    networks = health.networks,
                    lan = lan,
                    received = health.lan_beacons_received,
                    devices = devices.len()
                )
            );
            for d in &devices {
                if d.platform.is_empty() {
                    println!("      • {}", d.name);
                } else {
                    println!("      • {} ({})", d.name, d.platform);
                }
            }
            if let Some(hint) = health.hint() {
                println!("      {hint}");
            }
        }
        Err(e) => println!(
            "[✗] {}",
            t!("doctor.discovery_failed", error = format!("{e:#}"))
        ),
    }
    engine.close().await;
}

/// Whether an interface name looks like a VPN or tunnel (tun/tap, WireGuard,
/// NordVPN, Tailscale).
#[cfg(not(target_os = "android"))]
fn check_vpn_interfaces() -> bool {
    let Ok(interfaces) = network_interface::NetworkInterface::show() else {
        return false;
    };
    interfaces.iter().any(|iface| {
        let name = iface.name.to_lowercase();
        let is_vpn = ["tun", "tap", "nord", "wireguard", "wg", "tailscale"]
            .iter()
            .any(|marker| name.contains(marker));
        if is_vpn {
            info!("VPN interface detected: {}", iface.name);
        }
        is_vpn
    })
}

/// Android does not expose interface names reliably; VPN detection is skipped.
#[cfg(target_os = "android")]
fn check_vpn_interfaces() -> bool {
    false
}
