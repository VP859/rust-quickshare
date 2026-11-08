//! `ferry daemon`: receiving files in the background.
//!
//! Trusted devices with Auto-Accept are accepted automatically. Other offers,
//! texts and pairing requests are confirmed in the console when one is
//! attached, and rejected otherwise (e.g. when running as a system service).
//! Accepted texts are printed and copied to the clipboard when one is available.

use anyhow::Result;
use iroh::EndpointId;
use rust_i18n::t;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

use crate::cli::display::human_size;
use ferry::config::Config;
use ferry::discovery::{Discovery, DiscoveryOptions};
use ferry::localsend::{LocalSend, LocalSendOffer, LocalSendOptions};
use ferry::node::NodeEngine;
use ferry::pairing::PairRequest;
use ferry::protocol::messages::{BatchOffer, TextDecision, TransferOffer};
use ferry::transfer::risk;
use ferry::transfer::service::{
    self, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

/// [`TransferHandler`] of the daemon: Auto-Accept, console questions or rejection.
struct DaemonHandler {
    /// Folder from `--downloads-dir`; the configured one when `None`.
    downloads_dir: Option<PathBuf>,
    /// A console is attached, so questions can be asked.
    interactive: bool,
    /// Serializes console questions when offers arrive concurrently.
    prompt_lock: tokio::sync::Mutex<()>,
    /// On Linux the clipboard content disappears with this object, so it lives
    /// as long as the daemon.
    #[cfg(not(target_os = "android"))]
    clipboard: std::sync::Mutex<Option<arboard::Clipboard>>,
}

impl DaemonHandler {
    /// Asks a yes/no question on the console; anything but `y` means no.
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

    /// Puts `text` into the system clipboard; `false` when there is none (e.g. no display).
    fn copy_to_clipboard(&self, text: &str) -> bool {
        #[cfg(not(target_os = "android"))]
        {
            let Ok(mut clipboard) = self.clipboard.lock() else {
                return false;
            };
            if clipboard.is_none() {
                *clipboard = arboard::Clipboard::new().ok();
            }
            clipboard.as_mut().is_some_and(|c| c.set_text(text).is_ok())
        }

        #[cfg(target_os = "android")]
        {
            let _ = text;
            false
        }
    }
}

impl TransferHandler for DaemonHandler {
    /// Reloads the configuration, so `ferry config` and `ferry pairing` apply
    /// without a restart.
    async fn admits(&self, peer: EndpointId, pairing: bool) -> bool {
        Config::load().map_or(true, |c| c.admits(&peer.to_string(), pairing))
    }

    /// The other device removed this one: remove it here too.
    async fn peer_unpaired(&self, peer: EndpointId) {
        let removed = Config::load().and_then(|mut c| {
            let was = c.peers.remove(&peer.to_string()).is_some();
            if was {
                c.save()?;
            }
            Ok(was)
        });
        if removed.unwrap_or(false) {
            info!("{peer} removed this device; removed it from the trusted devices");
        }
    }

    /// Reloads the configuration for every offer, so `ferry peer` changes apply
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

        if config.auto_accepts_file(&peer.to_string(), &offer.file_name) {
            info!("Auto-Accept: '{}' from '{alias}'", offer.file_name);
            return OfferDecision::Accept { dest_dir };
        }
        let risk = risk::file_risk(&offer.file_name);
        if !self.interactive {
            // An Auto-Accept device sending a program or script is not trusted blindly.
            let reason = match (risk, peer_info.is_some_and(|p| p.auto_accept)) {
                (Some(_), true) => t!("daemon.no_auto_executable", alias = &alias),
                _ => t!("daemon.no_auto_accept", alias = &alias),
            };
            return OfferDecision::Reject {
                reason: reason.into_owned(),
            };
        }

        let question = t!(
            "daemon.offer",
            name = &offer.file_name,
            size = human_size(offer.file_size),
            alias = &alias,
            id = peer
        );
        if self
            .ask_user(format!("\n{}{question}", risk_warning(risk)))
            .await
        {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    /// LocalSend senders are not authenticated: always asked, never Auto-Accept.
    async fn decide_localsend(&self, offer: &LocalSendOffer) -> OfferDecision {
        let config = Config::load().unwrap_or_default();
        let dest_dir = self
            .downloads_dir
            .clone()
            .unwrap_or_else(|| config.download_dir());
        if !self.interactive {
            return OfferDecision::Reject {
                reason: t!("daemon.no_localsend").into_owned(),
            };
        }
        let question = t!(
            "localsend.offer",
            sender = offer.sender.label(),
            address = offer.sender.address,
            count = offer.count,
            size = human_size(offer.total_size),
            names = offer.names_preview()
        );
        if self
            .ask_user(format!("\n{}{question}", risk_warning(offer.risk)))
            .await
        {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    async fn decide_batch(&self, peer: EndpointId, batch: &BatchOffer) -> Option<OfferDecision> {
        let config = Config::load().unwrap_or_default();
        let dest_dir = self
            .downloads_dir
            .clone()
            .unwrap_or_else(|| config.download_dir());
        let peer_info = config.find_peer(&peer.to_string());
        let alias = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned());
        if config.auto_accepts_batch(&peer.to_string(), &batch.names) {
            info!("Auto-Accept: {} files from '{alias}'", batch.count);
            return Some(OfferDecision::Accept { dest_dir });
        }
        let risk = risk::worst_risk(&batch.names);
        if !self.interactive {
            let reason = match (risk, peer_info.is_some_and(|p| p.auto_accept)) {
                (Some(_), true) => t!("daemon.no_auto_executable", alias = &alias),
                _ => t!("daemon.no_auto_accept", alias = &alias),
            };
            return Some(OfferDecision::Reject {
                reason: reason.into_owned(),
            });
        }
        let question = t!(
            "daemon.batch",
            count = batch.count,
            size = human_size(batch.total_size),
            alias = &alias,
            names = batch.names.join(", ")
        );
        Some(
            if self
                .ask_user(format!("\n{}{question}", risk_warning(risk)))
                .await
            {
                OfferDecision::Accept { dest_dir }
            } else {
                OfferDecision::Reject {
                    reason: t!("transfer.user_rejected").into_owned(),
                }
            },
        )
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
            c.add_paired(
                req.peer,
                &req.name,
                req.addr,
                false,
                Some(req.discovery_key),
            )?;
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

    async fn receive_text(&self, peer: EndpointId, text: String) -> TextDecision {
        let config = Config::load().unwrap_or_default();
        let peer_info = config.find_peer(&peer.to_string());
        let alias = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned());
        if !config.auto_accepts_texts_from(&peer.to_string()) {
            if !self.interactive {
                return TextDecision::Rejected {
                    reason: t!("daemon.no_auto_text", alias = &alias).into_owned(),
                };
            }
            let question = t!(
                "daemon.text",
                alias = &alias,
                text = service::text_preview(&text, 200)
            );
            if !self.ask_user(format!("\n{question}")).await {
                return TextDecision::Rejected {
                    reason: t!("transfer.user_rejected").into_owned(),
                };
            }
        }
        let copied = self.copy_to_clipboard(&text);
        info!(
            "Text from '{alias}' ({} bytes), copied to the clipboard: {copied}",
            text.len()
        );
        if self.interactive {
            let header = if copied {
                t!("daemon.text_copied", alias = &alias)
            } else {
                t!("daemon.text_received", alias = &alias)
            };
            println!("\n{header}\n{text}\n");
        }
        TextDecision::Accepted
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
/// Warning line before a question about a file that can run code; empty for
/// harmless files.
fn risk_warning(risk: Option<risk::Risk>) -> String {
    risk.map_or_else(String::new, |r| format!("{}\n", r.warning()))
}

/// Creates `dir` if needed and checks that a file can be written there.
///
/// # Errors
///
/// Fails with a hint when the folder cannot be created or written to.
fn ensure_writable(dir: &std::path::Path) -> Result<()> {
    use anyhow::Context;
    std::fs::create_dir_all(dir)
        .and_then(|()| {
            tempfile::Builder::new()
                .prefix(".ferry-check")
                .tempfile_in(dir)
        })
        .map(drop)
        .with_context(|| t!("daemon.folder_not_writable", path = dir.display()))
}

/// # Errors
///
/// Fails when the node cannot start or the download folder is not writable.
pub async fn run_daemon(downloads_dir: Option<PathBuf>) -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    let ticket_str = engine.ticket().await?.serialize_to_string()?;
    let config = Config::load().unwrap_or_default();
    let target = downloads_dir
        .clone()
        .unwrap_or_else(|| config.download_dir());

    // Fail now, with the reason, instead of on the first file: a missing folder
    // that cannot be created, or one the service sandbox does not allow writing to.
    ensure_writable(&target)?;

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
        #[cfg(not(target_os = "android"))]
        clipboard: std::sync::Mutex::new(None),
    });

    let _discovery = Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::from_config(&config),
    )
    .inspect_err(|e| warn!("Local network discovery disabled: {e:#}"))
    .ok();

    // Kept alive until the daemon stops.
    let _localsend = if config.localsend {
        match LocalSend::start(
            &engine,
            LocalSendOptions::from_config(&config),
            handler.clone(),
        )
        .await
        {
            Ok(localsend) => {
                println!(" {}", t!("localsend.on", port = localsend.port()));
                Some(localsend)
            }
            Err(e) => {
                warn!("{e:#}");
                None
            }
        }
    } else {
        None
    };

    tokio::select! {
        _ = service::serve(engine.clone(), handler) => {}
        _ = tokio::signal::ctrl_c() => info!("Shutdown signal received"),
    }
    engine.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_writable;

    #[test]
    fn download_folder_is_checked_before_the_daemon_starts() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        // A missing folder is created, and the probe file does not stay behind.
        let new = dir.path().join("Pobrane");
        ensure_writable(&new).expect("created and writable");
        assert!(new.is_dir() && std::fs::read_dir(&new)?.next().is_none());
        // A path below a regular file can never be a folder.
        let file = dir.path().join("file");
        std::fs::write(&file, b"x")?;
        assert!(ensure_writable(&file.join("sub")).is_err());
        Ok(())
    }
}
