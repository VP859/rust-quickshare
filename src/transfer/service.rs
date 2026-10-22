//! Wspólna logika węzła dla wszystkich interfejsów (demon, TUI, GUI Tauri):
//! pętla przyjmowania połączeń, decyzja o ofercie, wysyłka do urządzeń i
//! pobieranie plików udostępnionych przez TICKET. Interfejsy dostarczają
//! jedynie [`TransferHandler`] i reagują na [`TransferEvent`].

use anyhow::{Context, Result};
use iroh::EndpointId;
use iroh::endpoint::Connection;
use serde::Serialize;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::Channel;
use super::receiver::{read_offer, receive_on_channel, reject};
use super::sender::{PreparedFile, SendOutcome, prepare_file, push_file, send_on_channel};
use crate::node::{NodeEngine, NodeTicket};
use crate::protocol::messages::{Hello, ShareToken, TransferOffer};

/// Kod zamknięcia połączenia, gdy token udostępnienia jest nieznany.
const CLOSE_UNKNOWN_SHARE: u32 = 2;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransferStatus {
    /// Liczenie haszu BLAKE3 przed wysłaniem.
    Preparing,
    /// Oferta czeka na decyzję użytkownika.
    AwaitingDecision,
    Running,
    Completed {
        path: Option<String>,
    },
    Rejected {
        reason: String,
    },
    Failed {
        error: String,
    },
}

/// Stan pojedynczego transferu – serializowany bezpośrednio do frontendu.
#[derive(Debug, Clone, Serialize)]
pub struct TransferEvent {
    pub id: String,
    pub peer: String,
    pub file_name: String,
    pub direction: Direction,
    pub total: u64,
    pub done: u64,
    pub status: TransferStatus,
}

pub type EventSink<'a> = &'a (dyn Fn(TransferEvent) + Send + Sync);

pub enum OfferDecision {
    Accept { dest_dir: PathBuf },
    Reject { reason: String },
}

/// Zachowanie zależne od interfejsu.
pub trait TransferHandler: Send + Sync + 'static {
    /// Decyzja o ofercie od `peer` (Auto-Accept, pytanie użytkownika, odrzucenie).
    fn decide(
        &self,
        peer: EndpointId,
        transfer_id: &str,
        offer: &TransferOffer,
    ) -> impl Future<Output = OfferDecision> + Send;

    /// Plik udostępniony pod tokenem z TICKETU (tryb "pull").
    fn shared_file(&self, _token: &ShareToken) -> Option<PreparedFile> {
        None
    }

    fn on_event(&self, event: TransferEvent);
}

/// Pomocnik emitujący kolejne stany jednego transferu.
pub struct Tracker<'a> {
    base: TransferEvent,
    sink: EventSink<'a>,
}

impl<'a> Tracker<'a> {
    pub fn new(
        peer: EndpointId,
        file_name: &str,
        total: u64,
        direction: Direction,
        sink: EventSink<'a>,
    ) -> Self {
        Self {
            base: TransferEvent {
                id: uuid::Uuid::new_v4().to_string(),
                peer: peer.to_string(),
                file_name: file_name.to_string(),
                direction,
                total,
                done: 0,
                status: TransferStatus::Preparing,
            },
            sink,
        }
    }

    pub fn id(&self) -> &str {
        &self.base.id
    }

    pub fn emit(&self, done: u64, status: TransferStatus) {
        let mut ev = self.base.clone();
        ev.done = done;
        ev.status = status;
        (self.sink)(ev);
    }

    pub fn progress(&self, done: u64) {
        self.emit(done, TransferStatus::Running);
    }

    fn finish<T>(&self, result: &Result<T>, path: impl FnOnce(&T) -> Option<String>) {
        match result {
            Ok(v) => self.emit(self.base.total, TransferStatus::Completed { path: path(v) }),
            Err(e) => self.emit(
                0,
                TransferStatus::Failed {
                    error: format!("{e:#}"),
                },
            ),
        }
    }
}

/// Główna pętla węzła: przyjmuje połączenia aż do zamknięcia endpointu.
pub async fn serve<H: TransferHandler>(engine: NodeEngine, handler: Arc<H>) {
    loop {
        let conn = match engine.accept().await {
            Ok(Some(conn)) => conn,
            Ok(None) => {
                info!("Endpoint zamknięty – koniec pętli nasłuchu");
                return;
            }
            Err(e) => {
                warn!("Nieudane połączenie przychodzące: {e:#}");
                continue;
            }
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            let peer = conn.remote_id();
            if let Err(e) = handle_connection(conn, handler).await {
                warn!("Błąd obsługi połączenia od {peer}: {e:#}");
            }
        });
    }
}

/// Jedno połączenie może przenosić wiele kolejnych transferów (np. wysyłka wielu plików).
async fn handle_connection<H: TransferHandler>(conn: Connection, handler: Arc<H>) -> Result<()> {
    while let Some((channel, hello)) = Channel::accept(&conn).await? {
        // Błąd jednego transferu nie zrywa połączenia – kolejne pliki mogą się powieść.
        let result = match hello {
            Hello::Push => handle_push(&conn, channel, handler.as_ref()).await,
            Hello::Pull { token } => handle_pull(&conn, channel, &token, handler.as_ref()).await,
        };
        if let Err(e) = result {
            warn!("Transfer od {} nieudany: {e:#}", conn.remote_id());
        }
    }
    debug!("Połączenie z {} zakończone", conn.remote_id());
    Ok(())
}

async fn handle_push<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    handler: &H,
) -> Result<()> {
    let peer = conn.remote_id();
    let offer = read_offer(&mut channel).await?;
    let sink = |e| handler.on_event(e);
    let tracker = Tracker::new(
        peer,
        &offer.file_name,
        offer.file_size,
        Direction::Download,
        &sink,
    );
    tracker.emit(0, TransferStatus::AwaitingDecision);

    match handler.decide(peer, tracker.id(), &offer).await {
        OfferDecision::Reject { reason } => {
            reject(channel, &reason).await?;
            tracker.emit(0, TransferStatus::Rejected { reason });
        }
        OfferDecision::Accept { dest_dir } => {
            let result =
                receive_on_channel(conn, channel, &offer, &dest_dir, &|d| tracker.progress(d))
                    .await;
            tracker.finish(&result, |p| Some(p.display().to_string()));
            result?;
        }
    }
    Ok(())
}

async fn handle_pull<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    token: &ShareToken,
    handler: &H,
) -> Result<()> {
    let Some(file) = handler.shared_file(token) else {
        warn!("{} zażądał nieznanego udostępnienia", conn.remote_id());
        conn.close(CLOSE_UNKNOWN_SHARE.into(), b"unknown share");
        return Ok(());
    };
    let sink = |e| handler.on_event(e);
    let tracker = Tracker::new(
        conn.remote_id(),
        &file.name,
        file.size,
        Direction::Upload,
        &sink,
    );
    let result = send_on_channel(conn, &mut channel, &file, &|d| tracker.progress(d)).await;
    report_send(&tracker, result)?;
    channel.finish().await;
    Ok(())
}

fn report_send(tracker: &Tracker<'_>, result: Result<SendOutcome>) -> Result<()> {
    match result {
        Ok(SendOutcome::Rejected(reason)) => {
            tracker.emit(0, TransferStatus::Rejected { reason });
            Ok(())
        }
        other => {
            tracker.finish(&other, |_| None);
            other.map(|_| ())
        }
    }
}

/// Wysyła pliki do urządzenia (jedno połączenie, kolejne strumienie kontrolne).
/// Błąd pojedynczego pliku nie przerywa wysyłki pozostałych.
pub async fn send_files(
    engine: &NodeEngine,
    target: iroh::EndpointAddr,
    paths: &[PathBuf],
    sink: EventSink<'_>,
) -> Result<()> {
    let peer = target.id;
    let conn = engine.connect(target).await?;
    let mut failures = 0usize;
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tracker = Tracker::new(peer, &name, 0, Direction::Upload, sink);
        tracker.emit(0, TransferStatus::Preparing);
        let result = async {
            let file = prepare_file(path).await?;
            push_file(&conn, &file, &|d| tracker.progress(d)).await
        }
        .await;
        if report_send(&tracker, result).is_err() {
            failures += 1;
        }
    }
    conn.close(0u32.into(), b"done");
    if failures > 0 {
        anyhow::bail!("Nie udało się wysłać {failures} z {} plików", paths.len());
    }
    Ok(())
}

/// Pobiera plik udostępniony przez TICKET z tokenem (strona łącząca się odbiera).
pub async fn pull_shared(
    engine: &NodeEngine,
    ticket: &NodeTicket,
    dest_dir: &Path,
    sink: EventSink<'_>,
) -> Result<PathBuf> {
    let token = ticket
        .share
        .context("TICKET nie zawiera udostępnionego pliku – to TICKET parowania urządzenia")?;
    let peer = ticket.addr.id;
    let conn = engine.connect(ticket.addr.clone()).await?;
    let mut channel = Channel::open(&conn, &Hello::Pull { token }).await?;
    let offer = read_offer(&mut channel)
        .await
        .context("Nadawca nie udostępnia już tego pliku")?;
    let tracker = Tracker::new(
        peer,
        &offer.file_name,
        offer.file_size,
        Direction::Download,
        sink,
    );
    let result =
        receive_on_channel(&conn, channel, &offer, dest_dir, &|d| tracker.progress(d)).await;
    tracker.finish(&result, |p| Some(p.display().to_string()));
    conn.close(0u32.into(), b"done");
    result
}

/// Generuje losowy token udostępnienia.
pub fn new_share_token() -> ShareToken {
    *uuid::Uuid::new_v4().as_bytes()
}
