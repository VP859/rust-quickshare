//! `ferry send`, `ferry text`, `ferry receive` and `ferry ticket`.

use anyhow::{Context, Result};
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use super::display::{TerminalProgress, print_ticket};
use ferry::config::Config;
use ferry::discovery::{Discovery, DiscoveryOptions};
use ferry::node::{NodeEngine, NodeTicket};
use ferry::protocol::messages::TextDecision;
use ferry::transfer::sender::prepare_file;
use ferry::transfer::service;
use ferry::transfer::share::{Share, ShareOptions, ShareSink};

/// How long to look for the receiver on the local network before sending.
const NEARBY_LOOKUP: Duration = Duration::from_millis(1500);

/// `ferry send <files> [--to <device>]`.
///
/// With `--to` the files are pushed to the device; otherwise a single file is
/// shared by a one-time TICKET until it has been downloaded once.
///
/// # Errors
///
/// Fails when the node cannot start, the receiver cannot be resolved or a transfer fails.
pub async fn run_send(files: &[PathBuf], to: Option<&str>) -> Result<()> {
    let Some(target) = to else {
        // Sharing by TICKET uses a throw-away identity, not the device's own.
        return share_by_ticket(files).await;
    };
    let engine = NodeEngine::with_persistent_identity().await?;
    let result = send_to_peer(&engine, files, target).await;
    engine.close().await;
    result
}

/// Address of the receiver; prefers a fresh address from local discovery.
/// `target` may also be the name of an unpaired nearby device.
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
        DiscoveryOptions::from_config(&config),
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

/// Pushes `files` to the device named by `target`, showing progress bars.
async fn send_to_peer(engine: &NodeEngine, files: &[PathBuf], target: &str) -> Result<()> {
    let addr = resolve_receiver(engine, target).await?;
    let progress = TerminalProgress::default();
    service::send_files(engine, addr, files, &|ev| progress.handle(ev)).await
}

/// `ferry text [TEXT] --to <device>`; without `TEXT` the text is read from
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

/// Shares one file by a one-time TICKET: a node with a new identity serves it
/// until it was downloaded once, and keeps serving after errors, so an
/// interrupted download can be resumed.
async fn share_by_ticket(files: &[PathBuf]) -> Result<()> {
    let [path] = files else {
        anyhow::bail!(t!("cli.share_single"));
    };
    println!("{}", t!("cli.hashing", name = path.display()));
    let file = prepare_file(path).await?;
    let mode = Config::load().unwrap_or_default().network_mode;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink: ShareSink = Arc::new(move |event| {
        let _ = tx.send(event);
    });
    let mut share = Share::start(
        file,
        mode,
        ShareOptions {
            single_use: true,
            lifetime: None,
        },
        sink,
    )
    .await?;
    print_ticket(
        &t!("cli.shared", name = share.file_name()),
        &share.ticket()?,
    );
    println!(
        "{}\n",
        t!("cli.share_identity", id = share.node_id().fmt_short())
    );

    let progress = TerminalProgress::default();
    // The channel closes when the share ends and drops its handler.
    while let Some(event) = rx.recv().await {
        progress.handle(event);
    }
    share.finished().await;
    Ok(())
}

/// `ferry receive <TICKET> [folder]`.
///
/// # Errors
///
/// Fails when the TICKET is invalid or the download fails.
pub async fn run_receive(ticket_str: &str, output_dir: Option<&Path>) -> Result<()> {
    let ticket = NodeTicket::deserialize_from_str(ticket_str)
        .with_context(|| t!("ticket.invalid_target"))?;
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

/// `ferry ticket`: prints this device's Node ID, TICKET and QR code.
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
