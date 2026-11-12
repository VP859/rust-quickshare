//! [`serve`]: accepts connections and answers what other devices ask for –
//! file offers (one by one or as a batch), downloads of shared files, pairing
//! and texts.

use super::events::{Cancelled, Direction, OfferDecision, TransferStatus};
use super::gate::{BatchApproval, QUESTION_TIMEOUT, QuestionGate, busy_reason};
use super::handler::TransferHandler;
use super::outgoing::{ignore_cancelled, is_retryable, report_send};
use super::tracker::{Tracker, discard_partial};
use crate::history::HistoryEntry;
use crate::node::NodeEngine;
use crate::pairing::{PAIR_DECISION_TIMEOUT, PairRequest, answer_pairing, clean_device_name};
use crate::protocol::codec::write_msg;
use crate::protocol::messages::{
    BatchDecision, BatchOffer, Hello, MAX_TEXT_LEN, PairResponse, ShareToken, TextDecision,
    TransferDecision,
};
use crate::transfer::Channel;
use crate::transfer::receiver::{
    check_free_space, part_path_for, read_offer, receive_on_channel, reject,
};
use crate::transfer::risk;
use crate::transfer::sender::send_on_channel;
use anyhow::Result;
use iroh::endpoint::Connection;
use rust_i18n::t;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Application close code sent when a download asks for an unknown share token.
const CLOSE_UNKNOWN_SHARE: u32 = 2;

/// Main node loop: accepts connections until the endpoint is closed.
///
/// Every connection is handled in its own task; it may carry several
/// consecutive transfers and a failure of one does not affect the others.
pub async fn serve<H: TransferHandler>(engine: NodeEngine, handler: Arc<H>) {
    let gate = Arc::new(QuestionGate::default());
    loop {
        let conn = match engine.accept().await {
            Ok(Some(conn)) => conn,
            Ok(None) => {
                info!("Endpoint closed, stopping the accept loop");
                return;
            }
            Err(e) => {
                warn!("Incoming connection failed: {e:#}");
                continue;
            }
        };
        let handler = handler.clone();
        let engine = engine.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            let peer = conn.remote_id();
            if let Err(e) = handle_connection(&engine, conn, handler, &gate).await {
                warn!("Error while handling the connection from {peer}: {e:#}");
            }
        });
    }
}

/// Serves every control stream of one incoming connection in turn.
async fn handle_connection<H: TransferHandler>(
    engine: &NodeEngine,
    conn: Connection,
    handler: Arc<H>,
    gate: &Arc<QuestionGate>,
) -> Result<()> {
    let peer = conn.remote_id();
    let mut approval = gate.take_batch(&peer);
    let result = serve_streams(engine, &conn, handler, gate, &mut approval).await;
    // A batch that was not finished stays accepted for a reconnection.
    if let Some(rest) = approval {
        gate.keep_batch(peer, rest);
    }
    result
}

/// Serves the control streams of `conn` until it closes.
async fn serve_streams<H: TransferHandler>(
    engine: &NodeEngine,
    conn: &Connection,
    handler: Arc<H>,
    gate: &Arc<QuestionGate>,
    approval: &mut Option<BatchApproval>,
) -> Result<()> {
    while let Some((channel, hello)) = Channel::accept(conn).await? {
        let pairing = matches!(hello, Hello::Pair { .. });
        // Downloads by TICKET are the holder's right: the token is the secret.
        if !matches!(hello, Hello::Pull { .. }) && !handler.admits(conn.remote_id(), pairing).await
        {
            info!(
                "Declined a request from the unknown device {}",
                conn.remote_id()
            );
            if let Err(e) = refuse(channel, &hello).await {
                debug!("Could not tell the device about the refusal: {e:#}");
            }
            continue;
        }
        let result = match hello {
            Hello::Push => handle_push(conn, channel, handler.as_ref(), gate, approval).await,
            Hello::Batch(batch) => handle_batch(conn, channel, &batch, handler.as_ref(), gate)
                .await
                .map(|granted| *approval = granted),
            Hello::Pull { token } => handle_pull(conn, channel, &token, handler.as_ref()).await,
            Hello::Pair {
                name,
                addr,
                commitment,
            } => {
                handle_pair(
                    engine,
                    conn,
                    channel,
                    &name,
                    addr,
                    &commitment,
                    handler.as_ref(),
                    gate,
                )
                .await
            }
            Hello::Text { text } => handle_text(conn, channel, text, handler.as_ref(), gate).await,
            Hello::Unpair => {
                handler.peer_unpaired(conn.remote_id()).await;
                channel.finish().await;
                Ok(())
            }
        };
        if let Err(e) = result {
            warn!("Transfer from {} failed: {e:#}", conn.remote_id());
        }
    }
    debug!("Connection with {} finished", conn.remote_id());
    Ok(())
}

/// Answers a request of a device that is not admitted ([`TransferHandler::admits`])
/// with a refusal that looks like the user's, and closes the stream. A pairing
/// request gets no answer at all, as when it arrives too often.
async fn refuse(mut channel: Channel, hello: &Hello) -> Result<()> {
    let reason = t!("service.paired_only").into_owned();
    match hello {
        Hello::Push => write_msg(&mut channel.send, &TransferDecision::Reject { reason }).await?,
        Hello::Batch(_) => {
            write_msg(&mut channel.send, &BatchDecision::Rejected { reason }).await?
        }
        Hello::Text { .. } => {
            write_msg(&mut channel.send, &TextDecision::Rejected { reason }).await?;
        }
        Hello::Pair { .. } | Hello::Pull { .. } | Hello::Unpair => return Ok(()),
    }
    channel.finish().await;
    Ok(())
}

/// Incoming file offer ([`Hello::Push`]): receives the file when it belongs to
/// an accepted batch, otherwise asks the handler first.
async fn handle_push<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    handler: &H,
    gate: &Arc<QuestionGate>,
    approval: &mut Option<BatchApproval>,
) -> Result<()> {
    let peer = conn.remote_id();
    let offer = read_offer(&mut channel).await?;
    let sink = |e| handler.on_event(e);
    let tracker = Tracker::new(
        peer,
        &offer.display_name(),
        offer.file_size,
        Direction::Download,
        &sink,
    )
    .with_connection(conn);
    // A batch approval does not cover files that can run code: the user saw at
    // most 20 names, and an Auto-Accept batch was never looked at. Those files
    // go through `decide` like single files, where the interface warns about them.
    let covered_by_batch = risk::file_risk(&offer.file_name).is_none();
    let accepted_before = gate.take_file(peer, &offer.file_hash).or_else(|| {
        covered_by_batch
            .then(|| approval.as_mut().and_then(|a| a.take(&offer)))
            .flatten()
    });
    let decision = match accepted_before {
        Some(dest_dir) => OfferDecision::Accept { dest_dir },
        None => match gate.enter(peer) {
            Some(_permit) => {
                tracker.emit(0, TransferStatus::AwaitingDecision);
                handler.decide(peer, tracker.id(), &offer).await
            }
            None => OfferDecision::Reject {
                reason: busy_reason(),
            },
        },
    };
    match decision {
        OfferDecision::Reject { reason } => {
            reject(channel, &reason).await?;
            tracker.emit(0, TransferStatus::Rejected { reason });
        }
        OfferDecision::Accept { dest_dir } => {
            let result = tracker
                .cancellable(receive_on_channel(conn, channel, &offer, &dest_dir, &|d| {
                    tracker.progress(d)
                }))
                .await;
            if let Err(e) = &result
                && is_retryable(e)
            {
                // The sender reconnects and resumes; the user already said yes.
                gate.keep_file(peer, offer.file_hash, dest_dir.clone());
            }
            discard_partial(&tracker, &result, part_path_for(&dest_dir, &offer)).await;
            tracker.finish(&result, |p| Some(p.display().to_string()));
            ignore_cancelled(result)?;
        }
    }
    Ok(())
}

/// Download of a file shared by TICKET ([`Hello::Pull`]); unknown tokens close the connection.
async fn handle_pull<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    token: &ShareToken,
    handler: &H,
) -> Result<()> {
    let Some(file) = handler.shared_file(token) else {
        warn!("{} requested an unknown share", conn.remote_id());
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
    )
    .with_connection(conn)
    .with_source(&file.path);
    let result = tracker
        .cancellable(send_on_channel(conn, &mut channel, &file, &|d| {
            tracker.progress(d)
        }))
        .await;
    if result.as_ref().is_err_and(|e| e.is::<Cancelled>()) {
        // The download ends for the other side as well.
        conn.close(0u32.into(), b"cancelled");
    }
    report_send(&tracker, result)?;
    channel.finish().await;
    Ok(())
}

/// Pairing request ([`Hello::Pair`]): exchanges the nonces, asks the handler
/// and answers with [`PairResponse`].
// The three fields of `Hello::Pair` are passed separately, like the other handlers.
#[allow(clippy::too_many_arguments)]
async fn handle_pair<H: TransferHandler>(
    engine: &NodeEngine,
    conn: &Connection,
    mut channel: Channel,
    name: &str,
    addr: iroh::EndpointAddr,
    commitment: &[u8; 32],
    handler: &H,
    gate: &Arc<QuestionGate>,
) -> Result<()> {
    let peer = conn.remote_id();
    // Dropping the stream without an answer ends the request on the other side
    // with "interrupted"; nothing is shown to the user here.
    if !gate.pairing_allowed(&peer) {
        anyhow::bail!("pairing request from {peer} ignored: retry delay after a declined request");
    }
    let Some(_permit) = gate.enter(peer) else {
        anyhow::bail!("pairing request from {peer} ignored: too many open requests");
    };
    let (code, discovery_key) =
        match answer_pairing(&mut channel, &engine.node_id(), &peer, commitment).await {
            Ok(answer) => answer,
            Err(e) => {
                gate.block_pairing(peer);
                return Err(e);
            }
        };
    let request = PairRequest {
        peer,
        name: clean_device_name(name),
        code,
        addr: (addr.id == peer).then_some(addr),
        discovery_key,
    };
    info!("Pairing request from '{}' ({peer})", request.name);
    let accepted = tokio::time::timeout(PAIR_DECISION_TIMEOUT, handler.confirm_pair(request))
        .await
        .unwrap_or(false);
    if !accepted {
        gate.block_pairing(peer);
    }
    let response = PairResponse {
        accepted,
        name: handler.device_name(),
        addr: accepted.then(|| engine.endpoint().addr()),
    };
    write_msg(&mut channel.send, &response).await?;
    channel.finish().await;
    Ok(())
}

/// Incoming text ([`Hello::Text`]): validates its size, asks the handler and
/// answers with [`TextDecision`].
async fn handle_text<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    text: String,
    handler: &H,
    gate: &Arc<QuestionGate>,
) -> Result<()> {
    let peer = conn.remote_id();
    let permit = gate.enter(peer);
    let text_len = text.len();
    let decision = if text.is_empty() || text.len() > MAX_TEXT_LEN {
        TextDecision::Rejected {
            reason: t!("service.text_too_long", max = MAX_TEXT_LEN).into_owned(),
        }
    } else if permit.is_none() {
        TextDecision::Rejected {
            reason: busy_reason(),
        }
    } else {
        info!("Text ({} bytes) from {peer}", text.len());
        tokio::time::timeout(QUESTION_TIMEOUT, handler.receive_text(peer, text))
            .await
            .unwrap_or_else(|_| TextDecision::Rejected {
                reason: t!("transfer.user_rejected").into_owned(),
            })
    };
    drop(permit);
    crate::history::record_text(HistoryEntry::text(
        Direction::Download,
        &peer.to_string(),
        text_len,
        match &decision {
            TextDecision::Accepted => Ok(()),
            TextDecision::Rejected { reason } => Err(("rejected", reason.clone())),
        },
    ));
    write_msg(&mut channel.send, &decision).await?;
    channel.finish().await;
    Ok(())
}

/// Announcement of several files ([`Hello::Batch`]): asks the handler once and
/// returns the approval for the files that follow, if they were accepted.
async fn handle_batch<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    batch: &BatchOffer,
    handler: &H,
    gate: &Arc<QuestionGate>,
) -> Result<Option<BatchApproval>> {
    batch.validate()?;
    let peer = conn.remote_id();
    let Some(_permit) = gate.enter(peer) else {
        let answer = BatchDecision::Rejected {
            reason: busy_reason(),
        };
        write_msg(&mut channel.send, &answer).await?;
        channel.finish().await;
        return Ok(None);
    };
    info!(
        "{} files ({} B) announced by {peer}",
        batch.count, batch.total_size
    );
    let decision = tokio::time::timeout(QUESTION_TIMEOUT, handler.decide_batch(peer, batch))
        .await
        .unwrap_or_else(|_| {
            Some(OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            })
        });
    let decision = match decision {
        Some(OfferDecision::Accept { dest_dir }) => {
            match check_free_space(&dest_dir, batch.total_size) {
                Ok(()) => Some(OfferDecision::Accept { dest_dir }),
                Err(reason) => Some(OfferDecision::Reject { reason }),
            }
        }
        other => other,
    };
    let (answer, approval) = match decision {
        None => (BatchDecision::AskEach, None),
        Some(OfferDecision::Accept { dest_dir }) => (
            BatchDecision::Accepted,
            Some(BatchApproval::new(batch.count, batch.total_size, dest_dir)),
        ),
        Some(OfferDecision::Reject { reason }) => (BatchDecision::Rejected { reason }, None),
    };
    write_msg(&mut channel.send, &answer).await?;
    channel.finish().await;
    Ok(approval)
}
