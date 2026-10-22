use anyhow::{Context, Result};
use iroh::EndpointId;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use super::display::{TerminalProgress, print_ticket};
use crate::config::Config;
use crate::discovery::{Discovery, DiscoveryOptions};
use crate::node::{NodeEngine, NodeTicket};
use crate::protocol::messages::{ShareToken, TransferOffer};
use crate::transfer::sender::{PreparedFile, prepare_file};
use crate::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

/// `rqs send <pliki> [--to <urządzenie>]`
pub async fn run_send(files: &[PathBuf], to: Option<&str>) -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    let result = match to {
        Some(target) => send_to_peer(&engine, files, target).await,
        None => share_by_ticket(&engine, files).await,
    };
    engine.close().await;
    result
}

/// Czas na znalezienie odbiorcy w sieci lokalnej przed wysyłką.
const NEARBY_LOOKUP: Duration = Duration::from_millis(1500);

async fn send_to_peer(engine: &NodeEngine, files: &[PathBuf], target: &str) -> Result<()> {
    let config = Config::load()?;
    let resolved = config.resolve_target(target);
    // Świeży adres z sieci lokalnej – transfer działa także bez dostępu do internetu,
    // a `--to` przyjmuje też nazwę niesparowanego urządzenia z `rqs discover`.
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
    println!("Łączenie z {}...", addr.id);
    let progress = TerminalProgress::default();
    service::send_files(engine, addr, files, &|ev| progress.handle(ev)).await
}

/// Handler trybu udostępniania: serwuje jeden plik pod tokenem, odrzuca wysyłki.
struct ShareHandler {
    token: ShareToken,
    file: PreparedFile,
    events: mpsc::UnboundedSender<TransferEvent>,
}

impl TransferHandler for ShareHandler {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Reject {
            reason: "Ten węzeł tylko udostępnia plik (użyj `rqs daemon`, aby odbierać)".into(),
        }
    }

    fn shared_file(&self, token: &ShareToken) -> Option<PreparedFile> {
        (token == &self.token).then(|| self.file.clone())
    }

    fn on_event(&self, event: TransferEvent) {
        let _ = self.events.send(event);
    }
}

async fn share_by_ticket(engine: &NodeEngine, files: &[PathBuf]) -> Result<()> {
    let [path] = files else {
        anyhow::bail!(
            "Udostępnianie przez TICKET obsługuje jeden plik – użyj --to dla wielu plików"
        );
    };
    println!("Obliczanie haszu BLAKE3 pliku {:?}...", path);
    let file = prepare_file(path).await?;
    let token = service::new_share_token();
    let ticket = NodeTicket::with_share(engine.node_addr().await?, token).serialize_to_string()?;
    print_ticket(
        &format!(
            "Udostępniono '{}'. Odbiorca: rqs receive <TICKET>",
            file.name
        ),
        &ticket,
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let handler = Arc::new(ShareHandler {
        token,
        file,
        events: tx,
    });
    let server = tokio::spawn(service::serve(engine.clone(), handler));

    // Serwujemy dalej po błędzie – odbiorca może wznowić pobieranie po zerwaniu połączenia.
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

/// `rqs receive <TICKET> [katalog]`
pub async fn run_receive(ticket_str: &str, output_dir: Option<&Path>) -> Result<()> {
    let ticket =
        NodeTicket::deserialize_from_str(ticket_str).context("Nieprawidłowy bilet połączeniowy")?;
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

/// `rqs ticket`
pub async fn run_show_ticket() -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    let ticket = engine.ticket().await?.serialize_to_string()?;
    println!("Node ID: {}", engine.node_id());
    print_ticket(
        "TICKET tego urządzenia – zeskanuj lub wklej w „+ Dodaj Urządzenie”",
        &ticket,
    );
    engine.close().await;
    Ok(())
}
