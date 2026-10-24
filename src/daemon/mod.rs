//! `rqs daemon`: receiving files in the background.
//!
//! Trusted devices with Auto-Accept are accepted automatically. Other offers
//! and pairing requests are confirmed in the console when one is attached, and
//! rejected otherwise (e.g. when running as a system service).

use anyhow::Result;
use iroh::EndpointId;
use rust_i18n::t;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

use crate::cli::display::human_size;
use crate::config::Config;
use crate::discovery::{Discovery, DiscoveryOptions};
use crate::node::NodeEngine;
use crate::pairing::PairRequest;
use crate::protocol::messages::TransferOffer;
use crate::transfer::service::{
    self, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

struct DaemonHandler {
    downloads_dir: Option<PathBuf>,
    interactive: bool,
    /// Serializes console questions when offers arrive concurrently.
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
    /// Reloads the configuration for every offer, so `rqs peer` changes apply
    /// without a restart.
    async fn decide(&self, peer: EndpointId, _: &str, offer: &TransferOffer) -> OfferDecision {
        let config = Config::load().unwrap_or_else(|e| {
            warn!("Could not load the configuration: {e:#}");
            Config::default()
        });
        let dest_dir = self
            .downloads_dir
            .clone()
            .unwrap_or_else(|| config.download_dir());
        let peer_info = config.find_peer(&peer.to_string());
        let alias = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned());

        if peer_info.is_some_and(|p| p.auto_accept) {
            info!("Auto-Accept: '{}' from '{alias}'", offer.file_name);
            return OfferDecision::Accept { dest_dir };
        }
        if !self.interactive {
            return OfferDecision::Reject {
                reason: t!("daemon.no_auto_accept", alias = &alias).into_owned(),
            };
        }

        let question = t!(
            "daemon.offer",
            name = &offer.file_name,
            size = human_size(offer.file_size),
            alias = &alias,
            id = peer
        );
        if self.ask_user(format!("\n{question}")).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    async fn confirm_pair(&self, req: PairRequest) -> bool {
        if !self.interactive {
            info!(
                "Rejected pairing with '{}': daemon has no terminal",
                req.name
            );
            return false;
        }
        let question = t!(
            "daemon.pair",
            name = &req.name,
            id = req.peer.fmt_short(),
            code = &req.code
        );
        if !self.ask_user(format!("\n{question}")).await {
            return false;
        }
        let saved = Config::load().and_then(|mut c| {
            c.add_paired(req.peer, &req.name, req.addr, false)?;
            c.save()
        });
        match saved {
            Ok(()) => {
                info!("Paired with '{}'", req.name);
                true
            }
            Err(e) => {
                warn!("Could not save the paired device: {e:#}");
                false
            }
        }
    }

    fn on_event(&self, ev: TransferEvent) {
        match ev.status {
            TransferStatus::Completed { path } => info!(
                "Received '{}' from {} → {}",
                ev.file_name,
                ev.peer,
                path.unwrap_or_default()
            ),
            TransferStatus::Failed { error } => {
                warn!("Transfer of '{}' failed: {error}", ev.file_name)
            }
            TransferStatus::Rejected { reason } => {
                info!("Rejected '{}' from {}: {reason}", ev.file_name, ev.peer)
            }
            _ => {}
        }
    }
}

/// Runs the daemon until Ctrl+C: announces the device on the local network and
/// accepts incoming transfers and pairing requests.
///
/// `downloads_dir` overrides the configured download folder.
///
/// # Errors
///
/// Fails when the node cannot start.
pub async fn run_daemon(downloads_dir: Option<PathBuf>) -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    let ticket_str = engine.ticket().await?.serialize_to_string()?;
    let config = Config::load().unwrap_or_default();
    let target = downloads_dir
        .clone()
        .unwrap_or_else(|| config.download_dir());

    println!("==================================================================");
    println!(" {}", t!("daemon.started"));
    println!(" Node ID : {}", engine.node_id());
    println!(" TICKET  : {ticket_str}");
    println!(" {}", t!("daemon.folder", path = target.display()));
    println!(" {}", t!("daemon.name", name = config.device_name()));
    println!("==================================================================\n");

    let handler = Arc::new(DaemonHandler {
        downloads_dir,
        interactive: std::io::stdin().is_terminal(),
        prompt_lock: tokio::sync::Mutex::new(()),
    });

    let _discovery = Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::default(),
    )
    .inspect_err(|e| warn!("Local network discovery disabled: {e:#}"))
    .ok();

    tokio::select! {
        _ = service::serve(engine.clone(), handler) => {}
        _ = tokio::signal::ctrl_c() => info!("Shutdown signal received"),
    }
    engine.close().await;
    Ok(())
}
