//! End-to-end tests of the RQS protocol on two real iroh nodes (QUIC).

use anyhow::{Context, Result};
use iroh::EndpointId;
use rqs_p2p::discovery::{Discovery, DiscoveryHealth, DiscoveryOptions};
use rqs_p2p::node::{NodeEngine, NodeTicket, load_or_generate_secret_key};
use rqs_p2p::pairing::{PairOutcome, PairRequest, pairing_code, request_pairing};
use rqs_p2p::protocol::codec::{read_msg, write_msg};
use rqs_p2p::protocol::messages::{
    DEFAULT_CHUNK_SIZE, Hello, MAX_TEXT_LEN, ShareToken, TextDecision, TransferDecision,
    TransferOffer,
};
use rqs_p2p::transfer::Channel;
use rqs_p2p::transfer::receiver::part_path_for;
use rqs_p2p::transfer::security::resolve_secure_target_path;
use rqs_p2p::transfer::sender::{PreparedFile, prepare_file};
use rqs_p2p::transfer::service::{
    self, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use std::path::{Path, PathBuf};
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

        let bad_offers = [
            ("../../.bashrc", DEFAULT_CHUNK_SIZE),
            ("/etc/passwd", DEFAULT_CHUNK_SIZE),
            ("ok.txt", 0),
            ("ok.txt", u32::MAX),
        ];
        for (name, chunk_size) in bad_offers {
            let conn = attacker.connect(addr.clone()).await?;
            let mut ch = Channel::open(&conn, &Hello::Push).await?;
            let offer = TransferOffer {
                file_name: name.into(),
                file_size: 10,
                chunk_size,
                file_hash: [0; 32],
            };
            write_msg(&mut ch.send, &offer).await?;
            let reply: Result<TransferDecision> = read_msg(&mut ch.recv).await;
            assert!(
                reply.is_err(),
                "offer {name:?}/{chunk_size} must be refused"
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

#[tokio::test(flavor = "multi_thread")]
async fn pairing_with_one_button_accept_and_reject() -> Result<()> {
    with_timeout(async {
        let dir = tempdir()?;
        for accept in [true, false] {
            let handler = Arc::new(TestHandler::new(accept, dir.path()));
            let (rx, addr) = start_receiver(handler.clone()).await?;
            let initiator = engine().await?;

            let outcome = request_pairing(&initiator, addr, "Laptop\nA").await?;
            let requests = handler.pair_requests.lock().unwrap().clone();
            assert_eq!(requests.len(), 1);
            let req = &requests[0];
            assert_eq!(req.peer, initiator.node_id());
            assert_eq!(req.name, "Laptop A");
            assert_eq!(req.addr.as_ref().map(|a| a.id), Some(initiator.node_id()));
            assert_eq!(req.code, pairing_code(&initiator.node_id(), &rx.node_id()));

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
