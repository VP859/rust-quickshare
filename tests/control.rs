//! Pausing transfers and the speed limit (`ferry::transfer::control`).
//!
//! The limit is global to the process, so these tests live in their own test
//! binary and run one after another.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ferry::node::NodeEngine;
use ferry::protocol::messages::TransferOffer;
use ferry::transfer::control::{set_speed_limit, speed_limit};
use ferry::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus, pause_transfer,
};
use iroh::{EndpointAddr, EndpointId, SecretKey, TransportAddr};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One test at a time: they change the process-wide limit.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct AcceptAll(PathBuf);

impl TransferHandler for AcceptAll {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Accept {
            dest_dir: self.0.clone(),
        }
    }
    fn on_event(&self, _: TransferEvent) {}
}

/// Sender, receiver serving into a temporary folder, and the receiver's
/// loopback address (Windows does not list loopback addresses itself).
async fn pair() -> (NodeEngine, NodeEngine, EndpointAddr, tempfile::TempDir) {
    let sender = NodeEngine::new(SecretKey::generate()).await.unwrap();
    let receiver = NodeEngine::new(SecretKey::generate()).await.unwrap();
    let dst = tempfile::tempdir().unwrap();
    tokio::spawn(service::serve(
        receiver.clone(),
        Arc::new(AcceptAll(dst.path().into())),
    ));
    let port = receiver
        .endpoint()
        .bound_sockets()
        .into_iter()
        .find(SocketAddr::is_ipv4)
        .unwrap()
        .port();
    let addr = EndpointAddr::from_parts(
        receiver.node_id(),
        [TransportAddr::Ip((Ipv4Addr::LOCALHOST, port).into())],
    );
    (sender, receiver, addr, dst)
}

fn file(dir: &std::path::Path, name: &str, size: usize) -> (PathBuf, Vec<u8>) {
    let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let path = dir.join(name);
    std::fs::write(&path, &data).unwrap();
    (path, data)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_speed_limit_slows_transfers_down() {
    let _serial = SERIAL.lock().await;
    let (sender, receiver, addr, dst) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let (path, data) = file(src.path(), "limited.bin", 3 * 1024 * 1024);

    set_speed_limit(Some(1024 * 1024));
    assert_eq!(speed_limit(), Some(1024 * 1024));
    let started = Instant::now();
    service::send_files(&sender, addr, &[path], &|_| {})
        .await
        .unwrap();
    let elapsed = started.elapsed();
    set_speed_limit(None);
    assert_eq!(speed_limit(), None);

    // 3 MiB at 1 MiB/s with a burst of one second: about 2 s.
    assert!(elapsed >= Duration::from_millis(1500), "took {elapsed:?}");
    assert!(elapsed < Duration::from_secs(20), "took {elapsed:?}");
    assert_eq!(std::fs::read(dst.path().join("limited.bin")).unwrap(), data);
    sender.close().await;
    receiver.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_transfer_waits_and_then_completes() {
    let _serial = SERIAL.lock().await;
    let (sender, receiver, addr, dst) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let (path, data) = file(src.path(), "paused.bin", 4 * 1024 * 1024);
    // Slow enough to pause in the middle.
    set_speed_limit(Some(2 * 1024 * 1024));

    let events: Arc<Mutex<Vec<TransferEvent>>> = Arc::default();
    let paused_at = Arc::new(Mutex::new(None::<(String, Instant)>));
    let send = {
        let (events, paused_at) = (events.clone(), paused_at.clone());
        async move {
            service::send_files(&sender, addr, &[path], &|e| {
                if e.status == TransferStatus::Running && e.done > 1024 * 1024 {
                    let mut paused = paused_at.lock().unwrap();
                    if paused.is_none() {
                        assert!(pause_transfer(&e.id, true));
                        *paused = Some((e.id.clone(), Instant::now()));
                    }
                }
                events.lock().unwrap().push(e);
            })
            .await
            .unwrap();
            sender
        }
    };
    let control = {
        let (events, paused_at) = (events.clone(), paused_at.clone());
        async move {
            let id = loop {
                if let Some((id, _)) = paused_at.lock().unwrap().clone() {
                    break id;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            // Let what was already on its way arrive (it is reported later on
            // a busy machine): wait until the progress stops changing, at most
            // 5 s, then watch for 1.5 s.
            let max_done =
                |events: &[TransferEvent]| events.iter().map(|e| e.done).max().unwrap_or(0);
            let mut before = max_done(&events.lock().unwrap());
            for _ in 0..25 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let now = max_done(&events.lock().unwrap());
                if now == before {
                    break;
                }
                before = now;
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let after = max_done(&events.lock().unwrap());
            assert!(
                after - before <= 256 * 1024,
                "no progress while paused: {before} → {after}"
            );
            assert!(pause_transfer(&id, false), "resumed");
        }
    };
    let (sender, ()) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(send, control)
    })
    .await
    .expect("finishes after resuming");
    set_speed_limit(None);

    let events = events.lock().unwrap().clone();
    let statuses: Vec<&TransferStatus> = events.iter().map(|e| &e.status).collect();
    let paused = statuses
        .iter()
        .position(|s| **s == TransferStatus::Paused)
        .expect("reported as paused");
    assert!(
        statuses[paused..].contains(&&TransferStatus::Running),
        "reported as running again"
    );
    assert!(matches!(
        statuses.last(),
        Some(TransferStatus::Completed { .. })
    ));
    assert!(events.iter().all(|e| e.direction == Direction::Upload));
    assert_eq!(std::fs::read(dst.path().join("paused.bin")).unwrap(), data);
    assert!(!pause_transfer("no-such-transfer", true));
    sender.close().await;
    receiver.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn finished_transfers_are_kept_in_the_history() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.jsonl");
    ferry::history::enable(Some(history.clone()));
    let (sender, receiver, addr, _dst) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let (path, _) = file(src.path(), "kept.txt", 1234);
    service::send_files(&sender, addr, std::slice::from_ref(&path), &|_| {})
        .await
        .unwrap();
    // The receiver finishes just after the sender got its confirmation.
    for _ in 0..100 {
        if ferry::history::load(&history).unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    ferry::history::enable(None);

    // Both ends run in this process: the upload and the download.
    let entries = ferry::history::load(&history).unwrap();
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(
        entries
            .iter()
            .all(|e| e.file_name == "kept.txt" && e.status == "completed" && e.size == 1234)
    );
    // Both can be sent again: the sent file and the received one.
    let upload = entries
        .iter()
        .find(|e| e.direction == Direction::Upload)
        .unwrap();
    assert_eq!(upload.local_file(), Some(path.clone()));
    assert_eq!(upload.ferry_peer(), Some(receiver.node_id()));
    let download = entries
        .iter()
        .find(|e| e.direction == Direction::Download)
        .unwrap();
    assert!(
        download
            .local_file()
            .is_some_and(|p| p.ends_with("kept.txt"))
    );
    assert_eq!(ferry::history::search(entries, "KEPT").len(), 2);

    // A text is recorded too – on both ends – but never its content.
    let secret = "my bank password is hunter2";
    ferry::history::enable(Some(history.clone()));
    let texted = tokio::time::timeout(
        Duration::from_secs(10),
        service::send_text(&sender, receiver_addr(&receiver), secret),
    )
    .await
    .unwrap();
    // This receiver does not take texts: the default handler declines.
    assert!(matches!(
        texted,
        Ok(ferry::protocol::messages::TextDecision::Rejected { .. })
    ));
    for _ in 0..100 {
        if ferry::history::load(&history).unwrap().len() == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    ferry::history::enable(None);
    let all = ferry::history::load(&history).unwrap();
    let texts: Vec<_> = all
        .iter()
        .filter(|e| e.kind == ferry::history::HistoryKind::Text)
        .collect();
    assert_eq!(texts.len(), 2, "{all:?}");
    assert!(
        texts
            .iter()
            .all(|e| e.status == "rejected" && e.size == secret.len() as u64)
    );
    assert!(
        !std::fs::read_to_string(&history)
            .unwrap()
            .contains("hunter2"),
        "the content is never written"
    );
    let entries = ferry::history::load(&history).unwrap();
    assert_eq!(entries.len(), 4);

    // Off: nothing more is written.
    let (path, _) = file(src.path(), "not-kept.txt", 10);
    service::send_files(&sender, receiver_addr(&receiver), &[path], &|_| {})
        .await
        .unwrap();
    assert_eq!(ferry::history::load(&history).unwrap().len(), 4);
    sender.close().await;
    receiver.close().await;
}

fn receiver_addr(receiver: &NodeEngine) -> EndpointAddr {
    let port = receiver
        .endpoint()
        .bound_sockets()
        .into_iter()
        .find(SocketAddr::is_ipv4)
        .unwrap()
        .port();
    EndpointAddr::from_parts(
        receiver.node_id(),
        [TransportAddr::Ip((Ipv4Addr::LOCALHOST, port).into())],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn files_go_to_several_devices_at_once() {
    let _serial = SERIAL.lock().await;
    let (sender, first, first_addr, first_dst) = pair().await;
    let (_, second, second_addr, second_dst) = pair().await;
    let src = tempfile::tempdir().unwrap();
    let (a, a_data) = file(src.path(), "a.bin", 700_000);
    let (b, b_data) = file(src.path(), "b.txt", 1000);

    let events: Mutex<Vec<TransferEvent>> = Mutex::default();
    let results = service::send_files_to_many(&sender, &[first_addr, second_addr], &[a, b], &|e| {
        events.lock().unwrap().push(e)
    })
    .await;
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    for dst in [&first_dst, &second_dst] {
        assert_eq!(std::fs::read(dst.path().join("a.bin")).unwrap(), a_data);
        assert_eq!(std::fs::read(dst.path().join("b.txt")).unwrap(), b_data);
    }
    let completed: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e.status, TransferStatus::Completed { .. }))
        .map(|e| e.peer.clone())
        .collect();
    assert_eq!(completed.len(), 4, "two files to each device");
    assert!(completed.contains(&first.node_id().to_string()));
    assert!(completed.contains(&second.node_id().to_string()));

    // An unreachable device fails alone.
    let gone = EndpointAddr::from_parts(
        SecretKey::generate().public(),
        [TransportAddr::Ip((Ipv4Addr::LOCALHOST, 9).into())],
    );
    let (c, _) = file(src.path(), "c.txt", 10);
    let results = tokio::time::timeout(
        Duration::from_secs(60),
        service::send_files_to_many(&sender, &[gone, receiver_addr(&first)], &[c], &|_| {}),
    )
    .await
    .unwrap();
    assert!(results[0].is_err() && results[1].is_ok());
    for node in [sender, first, second] {
        node.close().await;
    }
}

/// Receiver that cancels its download once 1 MiB arrived, keeping or
/// discarding the partial file.
struct CancelDownload {
    dir: PathBuf,
    discard: bool,
    finished: Mutex<Option<TransferStatus>>,
}

impl TransferHandler for CancelDownload {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Accept {
            dest_dir: self.dir.clone(),
        }
    }
    fn on_event(&self, e: TransferEvent) {
        if e.direction != Direction::Download {
            return;
        }
        if e.status == TransferStatus::Running && e.done > 1024 * 1024 {
            if self.discard {
                service::cancel_and_discard(&e.id);
            } else {
                service::cancel_transfer(&e.id);
            }
        }
        if e.status.is_final() {
            *self.finished.lock().unwrap() = Some(e.status);
        }
    }
}

/// Files in `dir` whose name ends with `.part`.
fn partial_files(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_download_keeps_its_partial_file_unless_it_is_discarded() {
    let _serial = SERIAL.lock().await;
    set_speed_limit(Some(1024 * 1024));
    for discard in [false, true] {
        let sender = NodeEngine::new(SecretKey::generate()).await.unwrap();
        let receiver = NodeEngine::new(SecretKey::generate()).await.unwrap();
        let dst = tempfile::tempdir().unwrap();
        let handler = Arc::new(CancelDownload {
            dir: dst.path().into(),
            discard,
            finished: Mutex::new(None),
        });
        tokio::spawn(service::serve(receiver.clone(), handler.clone()));
        let port = receiver
            .endpoint()
            .bound_sockets()
            .into_iter()
            .find(SocketAddr::is_ipv4)
            .unwrap()
            .port();
        let addr = EndpointAddr::from_parts(
            receiver.node_id(),
            [TransportAddr::Ip((Ipv4Addr::LOCALHOST, port).into())],
        );
        let src = tempfile::tempdir().unwrap();
        let (path, _) = file(src.path(), "big.bin", 4 * 1024 * 1024);
        let _ = service::send_files(&sender, addr, &[path], &|_| {}).await;
        // The receiver ends just after the sender saw its streams close.
        for _ in 0..100 {
            if handler.finished.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            *handler.finished.lock().unwrap(),
            Some(TransferStatus::Cancelled)
        );
        assert!(!dst.path().join("big.bin").exists());
        let partial = partial_files(dst.path());
        if discard {
            assert!(partial.is_empty(), "discarded: {partial:?}");
        } else {
            assert_eq!(partial.len(), 1, "kept for resuming: {partial:?}");
        }
        sender.close().await;
        receiver.close().await;
    }
    set_speed_limit(None);
}
