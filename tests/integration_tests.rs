//! End-to-end tests of the Ferry protocol on two real iroh nodes (QUIC).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use ferry::discovery::{Discovery, DiscoveryHealth, DiscoveryOptions, Visibility};
use ferry::node::{NetworkMode, NodeEngine, NodeTicket, load_or_generate_secret_key};
use ferry::pairing::{PairOutcome, PairRequest, request_pairing};
use ferry::protocol::codec::{read_msg, write_msg};
use ferry::protocol::messages::{
    BatchOffer, DEFAULT_CHUNK_SIZE, Hello, MAX_TEXT_LEN, PairChallenge, PairResponse, PairReveal,
    ShareToken, TextDecision, TransferDecision, TransferOffer,
};
use ferry::transfer::Channel;
use ferry::transfer::receiver::part_path_for;
use ferry::transfer::security::resolve_secure_target_path;
use ferry::transfer::sender::{PreparedFile, prepare_file};
use ferry::transfer::service::{
    self, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use iroh::EndpointId;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

const TEST_TIMEOUT: Duration = Duration::from_secs(60);

struct TestHandler {
    accept: bool,
    dest_dir: PathBuf,
    share: Option<(ShareToken, PreparedFile)>,
    events: Mutex<Vec<TransferEvent>>,
    pair_requests: Mutex<Vec<PairRequest>>,
    texts: Mutex<Vec<String>>,
    /// Answer to batches: `None` asks about every file, `Some(accept)` decides all.
    batch: Option<bool>,
    batches: Mutex<Vec<BatchOffer>>,
    decisions: AtomicUsize,
}

impl TestHandler {
    fn new(accept: bool, dest_dir: &Path) -> Self {
        Self {
            accept,
            dest_dir: dest_dir.to_path_buf(),
            share: None,
            events: Mutex::new(Vec::new()),
            pair_requests: Mutex::new(Vec::new()),
            texts: Mutex::new(Vec::new()),
            batch: None,
            batches: Mutex::new(Vec::new()),
            decisions: AtomicUsize::new(0),
        }
    }

    fn statuses(&self) -> Vec<TransferStatus> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.status.clone())
            .collect()
    }
}

impl TransferHandler for TestHandler {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        self.decisions.fetch_add(1, Ordering::SeqCst);
        if self.accept {
            OfferDecision::Accept {
                dest_dir: self.dest_dir.clone(),
            }
        } else {
            OfferDecision::Reject {
                reason: "test".into(),
            }
        }
    }

    fn shared_file(&self, token: &ShareToken) -> Option<PreparedFile> {
        self.share
            .as_ref()
            .filter(|(t, _)| t == token)
            .map(|(_, f)| f.clone())
    }

    fn on_event(&self, event: TransferEvent) {
        self.events.lock().unwrap().push(event);
    }

    fn device_name(&self) -> String {
        "Test Receiver".into()
    }

    async fn confirm_pair(&self, request: PairRequest) -> bool {
        self.pair_requests.lock().unwrap().push(request);
        self.accept
    }

    async fn decide_batch(&self, _: EndpointId, batch: &BatchOffer) -> Option<OfferDecision> {
        self.batches.lock().unwrap().push(batch.clone());
        self.batch.map(|accept| {
            if accept {
                OfferDecision::Accept {
                    dest_dir: self.dest_dir.clone(),
                }
            } else {
                OfferDecision::Reject {
                    reason: "batch".into(),
                }
            }
        })
    }

    async fn receive_text(&self, _: EndpointId, text: String) -> TextDecision {
        self.texts.lock().unwrap().push(text);
        if self.accept {
            TextDecision::Accepted
        } else {
            TextDecision::Rejected {
                reason: "test".into(),
            }
        }
    }
}

fn pattern(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

async fn engine() -> Result<NodeEngine> {
    NodeEngine::new(load_or_generate_secret_key(None)?).await
}

/// Starts a receiving node with `handler`; returns the engine and its address.
async fn start_receiver(handler: Arc<TestHandler>) -> Result<(NodeEngine, iroh::EndpointAddr)> {
    let receiver = engine().await?;
    let addr = receiver.node_addr().await?;
    tokio::spawn(service::serve(receiver.clone(), handler));
    Ok((receiver, addr))
}

async fn with_timeout<T>(fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(TEST_TIMEOUT, fut)
        .await
        .context("test timed out")?
}

#[tokio::test(flavor = "multi_thread")]
async fn push_multi_chunk_file_to_accepting_peer() -> Result<()> {
    with_timeout(async {
        let src = tempdir()?;
        let dst = tempdir()?;
        let data = pattern(5 * DEFAULT_CHUNK_SIZE as usize + 123);
        let path = src.path().join("report.bin");
        tokio::fs::write(&path, &data).await?;

        let handler = Arc::new(TestHandler::new(true, dst.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let sent = Mutex::new(Vec::new());
        service::send_files(&sender, addr, &[path], &|e| sent.lock().unwrap().push(e)).await?;

        assert_eq!(tokio::fs::read(dst.path().join("report.bin")).await?, data);
        let sent = sent.lock().unwrap();
        assert!(matches!(
            sent.last().map(|e| &e.status),
            Some(TransferStatus::Completed { .. })
        ));
        assert!(sent.iter().all(|e| e.total == data.len() as u64));
        Ok(())
    })
    .await
}

/// A folder arrives with its structure inside the download folder.
#[tokio::test(flavor = "multi_thread")]
async fn folder_is_sent_with_its_structure() -> Result<()> {
    with_timeout(async {
        let src = tempdir()?;
        let dst = tempdir()?;
        let root = src.path().join("Holiday");
        tokio::fs::create_dir_all(root.join("day 1")).await?;
        tokio::fs::write(root.join("notes.txt"), b"notes").await?;
        tokio::fs::write(root.join("day 1/photo.jpg"), pattern(3000)).await?;

        let handler = Arc::new(TestHandler::new(true, dst.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let sent = Mutex::new(Vec::new());
        service::send_files(&sender, addr, &[root], &|e| sent.lock().unwrap().push(e)).await?;

        let out = dst.path().join("Holiday");
        assert_eq!(tokio::fs::read(out.join("notes.txt")).await?, b"notes");
        assert_eq!(
            tokio::fs::read(out.join("day 1/photo.jpg")).await?,
            pattern(3000)
        );
        let names: Vec<_> = sent
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.file_name.clone())
            .collect();
        assert!(
            names.contains(&"Holiday/day 1/photo.jpg".to_string()),
            "{names:?}"
        );
        Ok(())
    })
    .await
}

/// Cancelling an upload in progress stops it without an error, reports it as
/// cancelled and leaves no finished file on the receiver.
#[tokio::test(flavor = "multi_thread")]
async fn upload_can_be_cancelled_while_running() -> Result<()> {
    with_timeout(async {
        let src = tempdir()?;
        let dst = tempdir()?;
        let path = src.path().join("large.bin");
        tokio::fs::write(&path, pattern(40 * DEFAULT_CHUNK_SIZE as usize)).await?;

        let handler = Arc::new(TestHandler::new(true, dst.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let sent = Mutex::new(Vec::new());
        service::send_files(&sender, addr, &[path], &|e| {
            if e.status == TransferStatus::Running {
                service::cancel_transfer(&e.id);
            }
            sent.lock().unwrap().push(e);
        })
        .await?;

        let sent = sent.lock().unwrap();
        let last = sent.last().context("no events")?;
        assert_eq!(last.status, TransferStatus::Cancelled);
        assert!(
            !service::cancel_transfer(&last.id),
            "a finished transfer cannot be cancelled"
        );
        assert!(!dst.path().join("large.bin").exists());
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn resumes_from_existing_part_file() -> Result<()> {
    with_timeout(async {
        let src = tempdir()?;
        let dst = tempdir()?;
        let data = pattern(5 * DEFAULT_CHUNK_SIZE as usize);
        let path = src.path().join("big_payload.dat");
        tokio::fs::write(&path, &data).await?;
        let prepared = prepare_file(&path).await?;

        let offer = TransferOffer {
            file_name: prepared.name.clone(),
            file_size: prepared.size,
            chunk_size: DEFAULT_CHUNK_SIZE,
            file_hash: prepared.hash,
            folder: Vec::new(),
        };
        let part = part_path_for(dst.path(), &offer)?;
        let resume_at = 2 * DEFAULT_CHUNK_SIZE as usize;
        tokio::fs::write(&part, &data[..resume_at + 1000]).await?;

        let handler = Arc::new(TestHandler::new(true, dst.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let sent = Mutex::new(Vec::new());
        service::send_files(&sender, addr, &[path], &|e| sent.lock().unwrap().push(e)).await?;

        assert_eq!(
            tokio::fs::read(dst.path().join("big_payload.dat")).await?,
            data
        );
        assert!(!part.exists(), "the .part file must be gone after success");
        let first_progress = sent
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.status == TransferStatus::Running)
            .map(|e| e.done);
        assert_eq!(first_progress, Some(resume_at as u64));
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn corrupted_part_file_is_detected_and_discarded() -> Result<()> {
    with_timeout(async {
        let src = tempdir()?;
        let dst = tempdir()?;
        let data = pattern(3 * DEFAULT_CHUNK_SIZE as usize);
        let path = src.path().join("x.bin");
        tokio::fs::write(&path, &data).await?;
        let prepared = prepare_file(&path).await?;
        let offer = TransferOffer {
            file_name: prepared.name.clone(),
            file_size: prepared.size,
            chunk_size: DEFAULT_CHUNK_SIZE,
            file_hash: prepared.hash,
            folder: Vec::new(),
        };
        let part = part_path_for(dst.path(), &offer)?;
        tokio::fs::write(&part, vec![0xEE; DEFAULT_CHUNK_SIZE as usize]).await?;

        let handler = Arc::new(TestHandler::new(true, dst.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let other = src.path().join("second.txt");
        tokio::fs::write(&other, b"ok").await?;
        let first =
            service::send_files(&sender, addr.clone(), &[path.clone(), other], &|_| {}).await;
        assert!(first.is_err(), "a corrupted .part file must be detected");
        assert!(!part.exists(), "a corrupted .part file must be deleted");
        assert_eq!(tokio::fs::read(dst.path().join("second.txt")).await?, b"ok");

        service::send_files(&sender, addr, &[path], &|_| {}).await?;
        assert_eq!(tokio::fs::read(dst.path().join("x.bin")).await?, data);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_offer_writes_nothing() -> Result<()> {
    with_timeout(async {
        let src = tempdir()?;
        let dst = tempdir()?;
        let path = src.path().join("secret.txt");
        tokio::fs::write(&path, b"data").await?;

        let handler = Arc::new(TestHandler::new(false, dst.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let sent = Mutex::new(Vec::new());
        service::send_files(&sender, addr, &[path], &|e| sent.lock().unwrap().push(e)).await?;

        assert!(matches!(
            sent.lock().unwrap().last().map(|e| &e.status),
            Some(TransferStatus::Rejected { .. })
        ));
        assert_eq!(std::fs::read_dir(dst.path())?.count(), 0);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_by_ticket_requires_matching_token() -> Result<()> {
    with_timeout(async {
        let src = tempdir()?;
        let dst = tempdir()?;
        let data = pattern(DEFAULT_CHUNK_SIZE as usize * 2 + 7);
        let path = src.path().join("photo-ünïcode.jpg");
        tokio::fs::write(&path, &data).await?;

        let token = service::new_share_token();
        let mut handler = TestHandler::new(false, dst.path());
        handler.share = Some((token, prepare_file(&path).await?));
        let handler = Arc::new(handler);
        let (_tx, addr) = start_receiver(handler.clone()).await?;
        let puller = engine().await?;

        let wrong = NodeTicket::with_share(addr.clone(), [0; 16]);
        assert!(
            service::pull_shared(&puller, &wrong, dst.path(), &|_| {})
                .await
                .is_err()
        );

        let ticket = NodeTicket::deserialize_from_str(
            &NodeTicket::with_share(addr, token).serialize_to_string()?,
        )?;
        let saved = service::pull_shared(&puller, &ticket, dst.path(), &|_| {}).await?;
        assert_eq!(saved.file_name().unwrap(), "photo-ünïcode.jpg");
        assert_eq!(tokio::fs::read(saved).await?, data);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            handler
                .statuses()
                .iter()
                .any(|s| matches!(s, TransferStatus::Completed { .. }))
        );
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn malicious_offers_are_refused() -> Result<()> {
    with_timeout(async {
        let dst = tempdir()?;
        let inner = dst.path().join("inbox");
        let handler = Arc::new(TestHandler::new(true, &inner));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let attacker = engine().await?;

        let folder = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let bad_offers = [
            ("../../.bashrc", DEFAULT_CHUNK_SIZE, vec![]),
            ("/etc/passwd", DEFAULT_CHUNK_SIZE, vec![]),
            ("ok.txt", 0, vec![]),
            ("ok.txt", u32::MAX, vec![]),
            ("ok.txt", DEFAULT_CHUNK_SIZE, folder(&[".."])),
            ("ok.txt", DEFAULT_CHUNK_SIZE, folder(&["a", "../../x"])),
            ("ok.txt", DEFAULT_CHUNK_SIZE, folder(&["/etc"])),
            ("ok.txt", DEFAULT_CHUNK_SIZE, folder(&["d"; 17])),
        ];
        for (name, chunk_size, folder) in bad_offers {
            let conn = attacker.connect(addr.clone()).await?;
            let mut ch = Channel::open(&conn, &Hello::Push).await?;
            let offer = TransferOffer {
                file_name: name.into(),
                file_size: 10,
                chunk_size,
                file_hash: [0; 32],
                folder: folder.clone(),
            };
            write_msg(&mut ch.send, &offer).await?;
            let reply: Result<TransferDecision> = read_msg(&mut ch.recv).await;
            assert!(
                reply.is_err(),
                "offer {name:?}/{chunk_size}/{folder:?} must be refused"
            );
        }
        assert!(!dst.path().join(".bashrc").exists());
        assert!(!inner.exists() || std::fs::read_dir(&inner)?.count() == 0);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_finds_peer_on_local_network() -> Result<()> {
    with_timeout(async {
        let a = engine().await?;
        let b = engine().await?;
        let (pa, pb) = (free_udp_port()?, free_udp_port()?);
        let opts = |listen: u16, to: u16| DiscoveryOptions {
            port: listen,
            targets: Some(vec![([127, 0, 0, 1], to).into()]),
            ..DiscoveryOptions::default()
        };
        let da = Discovery::start(a.clone(), "Laptop A", opts(pa, pb))?;
        let db = Discovery::start(b.clone(), "Phone B", opts(pb, pa))?;

        let seen_by_a = da
            .wait_for("phone b", Duration::from_secs(10))
            .await
            .context("A did not discover B")?;
        assert_eq!(seen_by_a.id, b.node_id());
        assert!(seen_by_a.addr.ip_addrs().next().is_some(), "no LAN address");
        let seen_by_b = db
            .wait_for(&a.node_id().to_string(), Duration::from_secs(10))
            .await
            .context("B did not discover A")?;
        assert_eq!(seen_by_b.name, "Laptop A");

        let mut health = db.health();
        for _ in 0..50 {
            if health.announced {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            health = db.health();
        }
        assert!(health.listening && health.lan_send_ok, "{health:?}");
        assert!(health.beacons_received > 0, "{health:?}");
        assert_eq!(
            health.lan_beacons_received, 0,
            "loopback is not the network"
        );
        assert!(health.hint().is_some(), "only this computer was heard");
        let heard_lan = DiscoveryHealth {
            lan_beacons_received: 1,
            ..health.clone()
        };
        assert_eq!(heard_lan.hint(), None);

        da.set_name("Office Laptop");
        assert!(
            db.wait_for("Office Laptop", Duration::from_secs(10))
                .await
                .is_some()
        );
        Ok(())
    })
    .await
}

/// A hidden device does not announce itself and answers only its paired
/// devices: the paired device sees it, a stranger on the same network does not.
#[tokio::test(flavor = "multi_thread")]
async fn hidden_device_is_seen_only_by_paired_devices() -> Result<()> {
    with_timeout(async {
        let (hidden, friend, stranger) = (engine().await?, engine().await?, engine().await?);
        let (ph, pf, ps) = (free_udp_port()?, free_udp_port()?, free_udp_port()?);
        let opts = |listen: u16, to: u16, visibility: Visibility| DiscoveryOptions {
            port: listen,
            targets: Some(vec![([127, 0, 0, 1], to).into()]),
            visibility,
        };
        let _dh = Discovery::start(
            hidden.clone(),
            "Hidden",
            opts(
                ph,
                pf,
                Visibility {
                    hidden: true,
                    trusted: [friend.node_id()].into(),
                },
            ),
        )?;
        let df = Discovery::start(
            friend.clone(),
            "Friend",
            opts(pf, ph, Visibility::default()),
        )?;
        let ds = Discovery::start(
            stranger.clone(),
            "Stranger",
            opts(ps, ph, Visibility::default()),
        )?;

        let hidden_id = hidden.node_id().to_string();
        assert!(
            df.wait_for(&hidden_id, Duration::from_secs(10))
                .await
                .is_some(),
            "the paired device sees the hidden one"
        );
        assert!(
            ds.wait_for(&hidden_id, Duration::from_secs(6))
                .await
                .is_none(),
            "a stranger does not"
        );
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_with_one_button_accept_and_reject() -> Result<()> {
    with_timeout(async {
        let dir = tempdir()?;
        for accept in [true, false] {
            let handler = Arc::new(TestHandler::new(accept, dir.path()));
            let (rx, addr) = start_receiver(handler.clone()).await?;
            let initiator = engine().await?;

            let shown = Mutex::new(None);
            let outcome = request_pairing(&initiator, addr, "Laptop\nA", |code| {
                *shown.lock().unwrap() = Some(code.to_string());
            })
            .await?;
            let requests = handler.pair_requests.lock().unwrap().clone();
            assert_eq!(requests.len(), 1);
            let req = &requests[0];
            assert_eq!(req.peer, initiator.node_id());
            assert_eq!(req.name, "Laptop A");
            assert_eq!(req.addr.as_ref().map(|a| a.id), Some(initiator.node_id()));
            let shown = shown
                .into_inner()
                .unwrap()
                .expect("code shown to the initiator");
            assert_eq!(req.code, shown, "both screens show the same code");

            match outcome {
                PairOutcome::Accepted { name, addr } => {
                    assert!(accept);
                    assert_eq!(name, "Test Receiver");
                    assert_eq!(addr.map(|a| a.id), Some(rx.node_id()));
                }
                PairOutcome::Rejected => assert!(!accept),
            }
        }
        Ok(())
    })
    .await
}

/// A requester whose revealed nonce does not match its commitment (e.g. an
/// attacker trying to choose the code) never reaches the user.
#[tokio::test(flavor = "multi_thread")]
async fn pairing_with_a_broken_commitment_is_dropped() -> Result<()> {
    with_timeout(async {
        let dir = tempdir()?;
        let handler = Arc::new(TestHandler::new(true, dir.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let attacker = engine().await?;

        let conn = attacker.connect(addr).await?;
        let hello = Hello::Pair {
            name: "Attacker".into(),
            addr: attacker.endpoint().addr(),
            commitment: [0; 32],
        };
        let mut channel = Channel::open(&conn, &hello).await?;
        let _challenge: PairChallenge = read_msg(&mut channel.recv).await?;
        write_msg(&mut channel.send, &PairReveal { nonce: [1; 32] }).await?;
        let response: Result<PairResponse> = read_msg(&mut channel.recv).await;
        assert!(response.is_err(), "no answer after a broken commitment");
        assert!(handler.pair_requests.lock().unwrap().is_empty());
        Ok(())
    })
    .await
}

/// Writes `count` small files and returns their paths and contents.
async fn write_files(dir: &Path, count: usize) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut files = Vec::new();
    for i in 0..count {
        let path = dir.join(format!("photo-{i}.jpg"));
        let data = pattern(1000 + i * 777);
        tokio::fs::write(&path, &data).await?;
        files.push((path, data));
    }
    Ok(files)
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_batch_is_confirmed_once() -> Result<()> {
    with_timeout(async {
        let (src, dst) = (tempdir()?, tempdir()?);
        let files = write_files(src.path(), 3).await?;
        let mut handler = TestHandler::new(false, dst.path());
        handler.batch = Some(true);
        let handler = Arc::new(handler);
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let paths: Vec<_> = files.iter().map(|(p, _)| p.clone()).collect();
        service::send_files(&sender, addr, &paths, &|_| {}).await?;

        let batches = handler.batches.lock().unwrap().clone();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].count, 3);
        assert_eq!(
            batches[0].total_size,
            files.iter().map(|(_, d)| d.len() as u64).sum::<u64>()
        );
        assert_eq!(
            handler.decisions.load(Ordering::SeqCst),
            0,
            "asked per file"
        );
        for (path, data) in &files {
            let name = path.file_name().unwrap();
            assert_eq!(&tokio::fs::read(dst.path().join(name)).await?, data);
        }
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_batch_sends_nothing() -> Result<()> {
    with_timeout(async {
        let (src, dst) = (tempdir()?, tempdir()?);
        let files = write_files(src.path(), 2).await?;
        let mut handler = TestHandler::new(true, dst.path());
        handler.batch = Some(false);
        let handler = Arc::new(handler);
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let paths: Vec<_> = files.iter().map(|(p, _)| p.clone()).collect();
        let sent = Mutex::new(Vec::new());
        service::send_files(&sender, addr, &paths, &|e| sent.lock().unwrap().push(e)).await?;

        assert_eq!(handler.decisions.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read_dir(dst.path())?.count(), 0);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(
            |e| matches!(&e.status, TransferStatus::Rejected { reason } if reason == "batch")
        ));
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_without_handler_support_asks_per_file() -> Result<()> {
    with_timeout(async {
        let (src, dst) = (tempdir()?, tempdir()?);
        let files = write_files(src.path(), 3).await?;
        let handler = Arc::new(TestHandler::new(true, dst.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let paths: Vec<_> = files.iter().map(|(p, _)| p.clone()).collect();
        service::send_files(&sender, addr, &paths, &|_| {}).await?;

        assert_eq!(handler.batches.lock().unwrap().len(), 1);
        assert_eq!(handler.decisions.load(Ordering::SeqCst), 3);
        assert_eq!(std::fs::read_dir(dst.path())?.count(), 3);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn text_is_delivered_or_declined() -> Result<()> {
    with_timeout(async {
        let dir = tempdir()?;
        let link = "https://example.org/path?q=1\nsecond line";
        for accept in [true, false] {
            let handler = Arc::new(TestHandler::new(accept, dir.path()));
            let (_rx, addr) = start_receiver(handler.clone()).await?;
            let sender = engine().await?;
            let decision = service::send_text(&sender, addr, link).await?;
            assert_eq!(handler.texts.lock().unwrap().as_slice(), [link]);
            assert_eq!(decision == TextDecision::Accepted, accept);
        }
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_text_is_refused_on_both_sides() -> Result<()> {
    with_timeout(async {
        let dir = tempdir()?;
        let handler = Arc::new(TestHandler::new(true, dir.path()));
        let (_rx, addr) = start_receiver(handler.clone()).await?;
        let sender = engine().await?;
        let too_long = "x".repeat(MAX_TEXT_LEN + 1);

        assert!(
            service::send_text(&sender, addr.clone(), &too_long)
                .await
                .is_err()
        );
        assert!(
            service::send_text(&sender, addr.clone(), "  ")
                .await
                .is_err()
        );

        let conn = sender.connect(addr).await?;
        let mut channel = Channel::open(&conn, &Hello::Text { text: too_long }).await?;
        let decision: TextDecision = read_msg(&mut channel.recv).await?;
        assert!(matches!(decision, TextDecision::Rejected { .. }));
        assert!(handler.texts.lock().unwrap().is_empty());
        Ok(())
    })
    .await
}

#[test]
fn text_preview_is_single_line_and_short() {
    assert_eq!(service::text_preview("a\n\tb   c\u{7}d", 50), "a b c d");
    assert_eq!(service::text_preview("abcdef", 4), "abc…");
}

/// Needs internet access: run with `cargo test -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the public iroh relays over the internet"]
async fn internet_mode_transfers_through_a_relay() -> Result<()> {
    with_timeout(async {
        let (src, dst) = (tempdir()?, tempdir()?);
        let data = pattern(3 * DEFAULT_CHUNK_SIZE as usize + 17);
        let path = src.path().join("relay.bin");
        tokio::fs::write(&path, &data).await?;

        let key = || load_or_generate_secret_key(None);
        let receiver = NodeEngine::with_mode(key()?, NetworkMode::Internet).await?;
        let addr = receiver.node_addr().await?;
        let relay_only = iroh::EndpointAddr::from_parts(
            addr.id,
            addr.relay_urls().cloned().map(iroh::TransportAddr::Relay),
        );
        assert!(relay_only.relay_urls().next().is_some(), "no relay URL");
        tokio::spawn(service::serve(
            receiver.clone(),
            Arc::new(TestHandler::new(true, dst.path())),
        ));

        let sender = NodeEngine::with_mode(key()?, NetworkMode::Internet).await?;
        service::send_files(&sender, relay_only, &[path], &|_| {}).await?;
        assert_eq!(tokio::fs::read(dst.path().join("relay.bin")).await?, data);
        Ok(())
    })
    .await
}

fn free_udp_port() -> Result<u16> {
    Ok(std::net::UdpSocket::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

#[tokio::test]
async fn test_path_traversal_exploit_blocked() -> Result<()> {
    let base_sandbox = tempdir()?;
    let result = resolve_secure_target_path(base_sandbox.path(), "../../.bashrc");
    assert!(
        result.is_err(),
        "path traversal protection accepted a malicious file name"
    );
    Ok(())
}
