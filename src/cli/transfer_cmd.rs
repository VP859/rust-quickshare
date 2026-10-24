//! `rqs send`, `rqs text`, `rqs receive` and `rqs ticket`.

use anyhow::{Context, Result};
use iroh::EndpointId;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use super::display::{TerminalProgress, print_ticket};
use crate::config::Config;
use crate::discovery::{Discovery, DiscoveryOptions};
use crate::node::{NodeEngine, NodeTicket};
use crate::protocol::messages::{ShareToken, TextDecision, TransferOffer};
use crate::transfer::sender::{PreparedFile, prepare_file};
use crate::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

/// How long to look for the receiver on the local network before sending.
const NEARBY_LOOKUP: Duration = Duration::from_millis(1500);

/// `rqs send <files> [--to <device>]`.
///
/// With `--to` the files are pushed to the device; otherwise a single file is
/// shared by a one-time TICKET until it has been downloaded once.
///
/// # Errors
///
/// Fails when the node cannot start, the receiver cannot be resolved or a transfer fails.
pub async fn run_send(files: &[PathBuf], to: Option<&str>) -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    let result = match to {
        Some(target) => send_to_peer(&engine, files, target).await,
        None => share_by_ticket(&engine, files).await,
    };
    engine.close().await;
    result
}

/// Address of the receiver; prefers a fresh address from local discovery (works
/// without internet). `target` may also be the name of an unpaired nearby device.
async fn resolve_receiver(engine: &NodeEngine, target: &str) -> Result<iroh::EndpointAddr> {
    let config = Config::load()?;
    let resolved = config.resolve_target(target);
    let query = match &resolved {
        Ok(addr) => addr.id.to_string(),
        Err(_) => target.to_string(),
    };
    let nearby = match Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::default(),
    ) {
        Ok(d) => d.wait_for(&query, NEARBY_LOOKUP).await,
        Err(_) => None,
    };
    let addr = match (nearby, resolved) {
        (Some(device), _) => device.addr,
        (None, Ok(addr)) => addr,
        (None, Err(e)) => return Err(e),
    };
    println!("{}", t!("cli.connecting", id = addr.id));
    Ok(addr)
}

async fn send_to_peer(engine: &NodeEngine, files: &[PathBuf], target: &str) -> Result<()> {
    let addr = resolve_receiver(engine, target).await?;
    let progress = TerminalProgress::default();
    service::send_files(engine, addr, files, &|ev| progress.handle(ev)).await
}

/// `rqs text [TEXT] --to <device>`; without `TEXT` the text is read from
/// standard input.
///
/// # Errors
///
/// Fails when the text is empty or too long, the receiver cannot be reached or
/// it declined the text.
pub async fn run_send_text(text: Option<String>, to: &str) -> Result<()> {
    let text = match text {
        Some(text) => text,
        None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            buf.trim_end_matches(['\r', '\n']).to_string()
        }
    };
    let engine = NodeEngine::with_persistent_identity().await?;
    let result = async {
        let addr = resolve_receiver(&engine, to).await?;
        println!("{}", t!("cli.text_waiting"));
        service::send_text(&engine, addr, &text).await
    }
    .await;
    engine.close().await;
    match result? {
        TextDecision::Accepted => {
            println!("{}", t!("cli.text_delivered"));
            Ok(())
        }
        TextDecision::Rejected { reason } => {
            anyhow::bail!(t!("cli.text_rejected", reason = reason))
        }
    }
}

/// Serves one file under a share token and rejects incoming offers.
struct ShareHandler {
    token: ShareToken,
    file: PreparedFile,
    events: mpsc::UnboundedSender<TransferEvent>,
}

impl TransferHandler for ShareHandler {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Reject {
            reason: t!("cli.share_only").into_owned(),
        }
    }

    fn shared_file(&self, token: &ShareToken) -> Option<PreparedFile> {
        (token == &self.token).then(|| self.file.clone())
    }

    fn on_event(&self, event: TransferEvent) {
        let _ = self.events.send(event);
    }
}

/// Keeps serving after errors, so the receiver can resume an interrupted download.
async fn share_by_ticket(engine: &NodeEngine, files: &[PathBuf]) -> Result<()> {
    let [path] = files else {
        anyhow::bail!(t!("cli.share_single"));
    };
    println!("{}", t!("cli.hashing", name = path.display()));
    let file = prepare_file(path).await?;
    let token = service::new_share_token();
    let ticket = NodeTicket::with_share(engine.node_addr().await?, token).serialize_to_string()?;
    print_ticket(&t!("cli.shared", name = &file.name), &ticket);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let handler = Arc::new(ShareHandler {
        token,
        file,
        events: tx,
    });
    let server = tokio::spawn(service::serve(engine.clone(), handler));

    let progress = TerminalProgress::default();
    while let Some(ev) = rx.recv().await {
        let delivered = ev.direction == Direction::Upload
            && matches!(ev.status, TransferStatus::Completed { .. });
        progress.handle(ev);
        if delivered {
            break;
        }
    }
    server.abort();
    Ok(())
}

/// `rqs receive <TICKET> [folder]`.
///
/// # Errors
///
/// Fails when the TICKET is invalid or the download fails.
pub async fn run_receive(ticket_str: &str, output_dir: Option<&Path>) -> Result<()> {
    let ticket =
        NodeTicket::deserialize_from_str(ticket_str).context(t!("ticket.invalid_target"))?;
    let dest_dir = match output_dir {
        Some(dir) => dir.to_path_buf(),
        None => Config::load().unwrap_or_default().download_dir(),
    };
    let engine = NodeEngine::with_persistent_identity().await?;
    let progress = TerminalProgress::default();
    let result = service::pull_shared(&engine, &ticket, &dest_dir, &|ev| progress.handle(ev)).await;
    engine.close().await;
    result.map(|_| ())
}

/// `rqs ticket`: prints this device's Node ID, TICKET and QR code.
///
/// # Errors
///
/// Fails when the node cannot start.
pub async fn run_show_ticket() -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    let ticket = engine.ticket().await?.serialize_to_string()?;
    println!("Node ID: {}", engine.node_id());
    print_ticket(&t!("cli.ticket_title"), &ticket);
    engine.close().await;
    Ok(())
}
