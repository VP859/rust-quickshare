use anyhow::Result;
#[cfg(not(target_os = "android"))]
use network_interface::NetworkInterfaceConfig;
use std::net::UdpSocket;
use tracing::info;

pub struct DoctorReport {
    pub udp_socket_ok: bool,
    pub stun_reachable: bool,
    pub vpn_interface_detected: bool,
    pub config_valid: bool,
}

pub async fn run_doctor() -> Result<DoctorReport> {
    println!("\n=== [RQS DOCTOR] Diagnostyka Środowiska Sieciowego ===");

    let udp_ok = match UdpSocket::bind("0.0.0.0:0") {
        Ok(_) => {
            println!("[✓] Gniazdo UDP: Można poprawnie otworzyć port UDP.");
            true
        }
        Err(e) => {
            println!("[✗] Gniazdo UDP: Błąd otwarcia gniazda: {}", e);
            false
        }
    };

    let stun_ok = check_stun_reachability().await;
    if stun_ok {
        println!("[✓] Serwery STUN: Dostępne i odpowiadają poprawnie.");
    } else {
        println!(
            "[!] Serwery STUN: Brak odpowiedzi. Połączenia mogą przechodzić przez Relay (DERP)."
        );
    }

    let vpn_detected = check_vpn_interfaces();
    if vpn_detected {
        println!(
            "[i] Wykryto aktywny interfejs VPN/Tunel. Może to wpływać na Direct UDP Hole Punching."
        );
    } else {
        println!("[✓] VPN: Brak aktywnych przeszkadzających interfejsów tunelowych.");
    }

    check_discovery().await;

    let config_ok = match crate::config::Config::load() {
        Ok(_) => {
            println!("[✓] Konfiguracja: Plik konfiguracyjny jest poprawny.\n");
            true
        }
        Err(e) => {
            println!("[✗] Konfiguracja: {e:#}\n");
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

/// Sprawdza wykrywanie urządzeń w sieci lokalnej (UDP broadcast/multicast, bez mDNS).
async fn check_discovery() {
    use crate::discovery::{DISCOVERY_PORT, Discovery, DiscoveryOptions};
    let config = crate::config::Config::load().unwrap_or_default();
    let engine = match crate::node::NodeEngine::with_persistent_identity().await {
        Ok(e) => e,
        Err(e) => {
            println!("[✗] Wykrywanie: nie uruchomiono węzła ({e:#})");
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
            println!(
                "[{}] Wykrywanie (UDP {DISCOVERY_PORT}): sieci lokalne: {}, wysyłanie do LAN: {}, odebrane ogłoszenia: {}, urządzenia w pobliżu: {}",
                if health.hint().is_none() { "✓" } else { "!" },
                health.networks,
                if health.lan_send_ok {
                    "OK"
                } else {
                    "ZABLOKOWANE"
                },
                health.beacons_received,
                devices.len()
            );
            for d in &devices {
                println!("      • {} ({})", d.name, d.platform);
            }
            if let Some(hint) = health.hint() {
                println!("      {hint}");
            }
        }
        Err(e) => println!("[✗] Wykrywanie: {e:#}"),
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
fn check_vpn_interfaces() -> bool {
    #[cfg(not(target_os = "android"))]
    {
        if let Ok(interfaces) = network_interface::NetworkInterface::show() {
            for iface in interfaces {
                let name = iface.name.to_lowercase();
                if name.contains("tun")
                    || name.contains("tap")
                    || name.contains("nord")
                    || name.contains("wireguard")
                    || name.contains("tailscale")
                {
                    info!("Wykryto interfejs VPN: {}", iface.name);
                    return true;
                }
            }
        }
        false
    }

    #[cfg(target_os = "android")]
    {
        // Na Androidzie aplikacja nie ma dostępu do listy interfejsów przez
        // netlink/getifaddrs w sandboxie NDK. Wykrywaniem VPN zajmuje się
        // warstwa natywna (ConnectivityManager → plugin JNI `is_vpn_active`),
        // a iroh i tak korzysta z Connection Migration (QUIC) przy zmianie sieci.
        info!("Android: wykrywanie VPN delegowane do ConnectivityManager (JNI).");
        false
    }
}
