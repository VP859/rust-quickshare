//! `rqs doctor`: diagnostics of the network environment and configuration.

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
    /// A public STUN server is reachable (direct connections are likely to work).
    pub stun_reachable: bool,
    /// A VPN or tunnel interface is active.
    pub vpn_interface_detected: bool,
    /// `config.toml` is missing or parses correctly.
    pub config_valid: bool,
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

    let stun_ok = check_stun_reachability().await;
    if stun_ok {
        println!("[✓] {}", t!("doctor.stun_ok"));
    } else {
        println!("[!] {}", t!("doctor.stun_failed"));
    }

    let vpn_detected = check_vpn_interfaces();
    if vpn_detected {
        println!("[i] {}", t!("doctor.vpn_found"));
    } else {
        println!("[✓] {}", t!("doctor.vpn_none"));
    }

    check_discovery().await;

    let config_ok = match crate::config::Config::load() {
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
        stun_reachable: stun_ok,
        vpn_interface_detected: vpn_detected,
        config_valid: config_ok,
    })
}

async fn check_discovery() {
    use crate::discovery::{DISCOVERY_PORT, Discovery, DiscoveryOptions};
    let config = crate::config::Config::load().unwrap_or_default();
    let engine = match crate::node::NodeEngine::with_persistent_identity().await {
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
        DiscoveryOptions::default(),
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
                    received = health.beacons_received,
                    devices = devices.len()
                )
            );
            for d in &devices {
                println!("      • {} ({})", d.name, d.platform);
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

async fn check_stun_reachability() -> bool {
    if let Ok(socket) = tokio::net::UdpSocket::bind("0.0.0.0:0").await {
        socket.connect("108.177.14.127:19302").await.is_ok()
    } else {
        false
    }
}

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

#[cfg(target_os = "android")]
fn check_vpn_interfaces() -> bool {
    false
}
