//! Sharing a file by TICKET with a one-time identity (`transfer::share`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferry::node::{NetworkMode, NodeEngine, NodeTicket, SHARE_PORTS};
use ferry::transfer::sender::prepare_file;
use ferry::transfer::service::{self, TransferEvent};
use ferry::transfer::share::{Share, ShareOptions};
use iroh::SecretKey;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Writes a file of `size` bytes and prepares it for sharing.
async fn file(
    dir: &std::path::Path,
    name: &str,
    size: usize,
) -> (ferry::transfer::sender::PreparedFile, Vec<u8>) {
    let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let path = dir.join(name);
    tokio::fs::write(&path, &data).await.unwrap();
    (prepare_file(&path).await.unwrap(), data)
}

fn quiet() -> ferry::transfer::share::ShareSink {
    Arc::new(|_| {})
}

async fn client() -> NodeEngine {
    NodeEngine::new(SecretKey::generate()).await.unwrap()
}

/// Downloads the shared file through `ticket` into `dest`.
async fn download(ticket: &str, dest: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let ticket = NodeTicket::deserialize_from_str(ticket)?;
    let engine = client().await;
    let result = service::pull_shared(&engine, &ticket, dest, &|_| {}).await;
    engine.close().await;
    result
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_has_its_own_identity_and_the_file_arrives() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (prepared, data) = file(src.path(), "report.bin", 3 * 1024 * 1024 + 5).await;

    // The device's permanent identity, which must not appear in the TICKET.
    let device = SecretKey::generate().public();

    let events = Arc::new(Mutex::new(Vec::<TransferEvent>::new()));
    let sink = {
        let events = events.clone();
        Arc::new(move |e| events.lock().unwrap().push(e)) as ferry::transfer::share::ShareSink
    };
    let share = Share::start(prepared, NetworkMode::Local, ShareOptions::default(), sink)
        .await
        .unwrap();
    let ticket = share.ticket().unwrap();
    let parsed = NodeTicket::deserialize_from_str(&ticket).unwrap();
    assert_eq!(parsed.addr.id, share.node_id());
    assert_ne!(parsed.addr.id, device);
    assert!(parsed.share.is_some());

    let saved = download(&ticket, dst.path()).await.unwrap();
    assert_eq!(tokio::fs::read(saved).await.unwrap(), data);
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e.status, service::TransferStatus::Completed { .. })),
        "the sender is told about the delivery"
    );
    share.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn every_share_is_a_different_identity() {
    let src = tempfile::tempdir().unwrap();
    let (prepared, _) = file(src.path(), "a.bin", 1000).await;
    let a = Share::start(
        prepared.clone(),
        NetworkMode::Local,
        ShareOptions::default(),
        quiet(),
    )
    .await
    .unwrap();
    let b = Share::start(
        prepared,
        NetworkMode::Local,
        ShareOptions::default(),
        quiet(),
    )
    .await
    .unwrap();
    assert_ne!(a.node_id(), b.node_id(), "two shares cannot be linked");

    // Each listens in the share port range, next to the main port.
    let ticket = NodeTicket::deserialize_from_str(&a.ticket().unwrap()).unwrap();
    let ports: Vec<u16> = ticket
        .addr
        .ip_addrs()
        .map(std::net::SocketAddr::port)
        .collect();
    assert!(
        !ports.is_empty() && ports.iter().all(|p| SHARE_PORTS.contains(p)),
        "{ports:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_single_use_share_ends_after_the_download() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (prepared, data) = file(src.path(), "once.bin", 200_000).await;
    let mut share = Share::start(
        prepared,
        NetworkMode::Local,
        ShareOptions {
            single_use: true,
            ..ShareOptions::default()
        },
        quiet(),
    )
    .await
    .unwrap();
    let ticket = share.ticket().unwrap();

    let saved = download(&ticket, dst.path()).await.unwrap();
    assert_eq!(tokio::fs::read(saved).await.unwrap(), data);
    tokio::time::timeout(Duration::from_secs(10), share.finished())
        .await
        .expect("the share ends by itself");
    assert!(share.is_finished());

    // Nobody listens any more: a second download fails.
    let again = tokio::time::timeout(Duration::from_secs(8), download(&ticket, dst.path())).await;
    assert!(!matches!(again, Ok(Ok(_))), "no second download");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_expires() {
    let src = tempfile::tempdir().unwrap();
    let (prepared, _) = file(src.path(), "short.bin", 1000).await;
    let mut share = Share::start(
        prepared,
        NetworkMode::Local,
        ShareOptions {
            lifetime: Some(Duration::from_millis(300)),
            ..ShareOptions::default()
        },
        quiet(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), share.finished())
        .await
        .expect("the share expires");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_declines_files_sent_to_it() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (prepared, _) = file(src.path(), "shared.bin", 1000).await;
    let share = Share::start(
        prepared,
        NetworkMode::Local,
        ShareOptions::default(),
        quiet(),
    )
    .await
    .unwrap();
    let target = NodeTicket::deserialize_from_str(&share.ticket().unwrap())
        .unwrap()
        .addr;

    let send_dir = tempfile::tempdir().unwrap();
    file(send_dir.path(), "evil.bin", 1000).await;
    let sender = client().await;
    let rejected = Mutex::new(false);
    service::send_files(&sender, target, &[send_dir.path().join("evil.bin")], &|e| {
        if matches!(e.status, service::TransferStatus::Rejected { .. }) {
            *rejected.lock().unwrap() = true;
        }
    })
    .await
    .unwrap();
    assert!(*rejected.lock().unwrap(), "the offer was declined");
    assert!(!dst.path().join("evil.bin").exists());
    share.stop();
}
