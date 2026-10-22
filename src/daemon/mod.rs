use anyhow::Result;
use iroh::EndpointId;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::Config;
use crate::discovery::{Discovery, DiscoveryOptions};
use crate::node::NodeEngine;
use crate::pairing::PairRequest;
use crate::protocol::messages::TransferOffer;
use crate::transfer::service::{
    self, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

/// Demon: zaufane urządzenia z Auto-Accept są przyjmowane automatycznie; pozostałe –
/// po potwierdzeniu w konsoli (jeśli jest terminal) lub odrzucane (usługa systemowa).
struct DaemonHandler {
    downloads_dir: Option<PathBuf>,
    interactive: bool,
    /// Serializuje pytania w konsoli przy równoległych ofertach.
    prompt_lock: tokio::sync::Mutex<()>,
}

impl DaemonHandler {
    async fn ask_user(&self, question: String) -> bool {
        let _guard = self.prompt_lock.lock().await;
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            print!("{question} [y/N]: ");
            let _ = std::io::stdout().flush();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).is_ok() && input.trim().eq_ignore_ascii_case("y")
        })
        .await
        .unwrap_or(false)
    }
}

impl TransferHandler for DaemonHandler {
    async fn decide(&self, peer: EndpointId, _: &str, offer: &TransferOffer) -> OfferDecision {
        // Konfiguracja czytana przy każdej ofercie – zmiany z `rqs peer` działają bez restartu.
        let config = Config::load().unwrap_or_else(|e| {
            warn!("Nie udało się wczytać konfiguracji: {e:#}");
            Config::default()
        });
        let dest_dir = self
            .downloads_dir
            .clone()
            .unwrap_or_else(|| config.download_dir());
        let peer_info = config.find_peer(&peer.to_string());
        let alias = peer_info
            .map(|p| p.alias.as_str())
            .unwrap_or("Nieznane Urządzenie");

        if peer_info.is_some_and(|p| p.auto_accept) {
            info!("Auto-Accept: '{}' od '{}'", offer.file_name, alias);
            return OfferDecision::Accept { dest_dir };
        }
        if !self.interactive {
            return OfferDecision::Reject {
                reason: format!("Urządzenie {alias} nie ma włączonego Auto-Accept"),
            };
        }

        let question = format!(
            "\n[OFERTA] '{}' ({:.2} MB) od '{}' ({}). Odebrać?",
            offer.file_name,
            offer.file_size as f64 / 1_048_576.0,
            alias,
            peer
        );
        if self.ask_user(question).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: "Użytkownik odrzucił transfer".into(),
            }
        }
    }

    async fn confirm_pair(&self, req: PairRequest) -> bool {
        if !self.interactive {
            info!("Odrzucono parowanie z '{}' – demon bez terminala", req.name);
            return false;
        }
        let question = format!(
            "\n[PAROWANIE] '{}' ({}) chce się sparować.\nKod weryfikacyjny: {} – czy jest taki sam na drugim urządzeniu?",
            req.name,
            req.peer.fmt_short(),
            req.code
        );
        if !self.ask_user(question).await {
            return false;
        }
        let saved = Config::load().and_then(|mut c| {
            c.add_paired(req.peer, &req.name, req.addr, false)?;
            c.save()
        });
        match saved {
            Ok(()) => {
                info!("Sparowano z '{}'", req.name);
                true
            }
            Err(e) => {
                warn!("Nie udało się zapisać sparowanego urządzenia: {e:#}");
                false
            }
        }
    }

    fn on_event(&self, ev: TransferEvent) {
        match ev.status {
            TransferStatus::Completed { path } => info!(
                "[SUKCES] '{}' od {} → {}",
                ev.file_name,
                ev.peer,
                path.unwrap_or_default()
            ),
            TransferStatus::Failed { error } => warn!("[BŁĄD] '{}': {error}", ev.file_name),
            TransferStatus::Rejected { reason } => {
                info!("[ODRZUCONO] '{}' od {}: {reason}", ev.file_name, ev.peer)
            }
            _ => {}
        }
    }
}

pub async fn run_daemon(downloads_dir: Option<PathBuf>) -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    let ticket_str = engine.ticket().await?.serialize_to_string()?;
    let target = downloads_dir
        .clone()
        .unwrap_or_else(|| Config::load().unwrap_or_default().download_dir());

    println!("==================================================================");
    println!(" DEMON RQS URUCHOMIONY");
    println!(" Twój Node ID: {}", engine.node_id());
    println!(" Twój TICKET : {}", ticket_str);
    println!(" Katalog pobierania: {:?}", target);
    println!("==================================================================\n");

    let handler = Arc::new(DaemonHandler {
        downloads_dir,
        interactive: std::io::stdin().is_terminal(),
        prompt_lock: tokio::sync::Mutex::new(()),
    });

    // Ogłaszanie w sieci lokalnej – inne urządzenia widzą demona i mogą się sparować.
    let config = Config::load().unwrap_or_default();
    let _discovery = Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::default(),
    )
    .inspect_err(|e| warn!("Wykrywanie w sieci lokalnej wyłączone: {e:#}"))
    .ok();
    println!(" Nazwa w sieci: {}\n", config.device_name());

    tokio::select! {
        _ = service::serve(engine.clone(), handler) => {}
        _ = tokio::signal::ctrl_c() => info!("Otrzymano sygnał zakończenia"),
    }
    engine.close().await;
    Ok(())
}
