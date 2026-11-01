//! Transfers over a bad network: packet loss and delay, a change of network in
//! the middle of a transfer, and the link going away for a while (a phone
//! falling asleep, Wi-Fi dropping).
//!
//! The conditions come from [`LossyLink`], a UDP forwarder between the two
//! nodes inside the test, so no root rights or `tc netem` are needed. The
//! sender only knows the forwarder's address and the receiver sees all
//! packets coming from it, as from a NAT. On one computer iroh would find the
//! direct path between the nodes at once (hole punching), so both nodes use a
//! [`ThroughLinkOnly`] path selector that never picks it. Tests that wait for
//! the QUIC idle timeout (30 s) are `#[ignore]`d and run in CI's network job:
//! `cargo test --test network_conditions -- --include-ignored`. The outage and
//! network-change tests are also ignored on Windows, where they time out on
//! slow CI runners; they check QUIC behaviour that does not depend on the
//! system and run on Linux in every CI build.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use ferry::node::engine::ALPN;
use ferry::node::{NetworkMode, NodeEngine};
use ferry::protocol::messages::{BatchOffer, TransferOffer};
use ferry::transfer::service::{
    self, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use iroh::endpoint::transports::{FourTuple, PathSelection, PathSelectionContext, PathSelector};
use iroh::endpoint::{Endpoint, presets};
use iroh::{EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// Network conditions applied by a [`LossyLink`]; can change at any time.
#[derive(Default)]
struct Conditions {
    /// Dropped packets per thousand, in both directions.
    loss_permille: AtomicU32,
    /// Added one-way delay in milliseconds.
    delay_ms: AtomicU64,
    /// Drop everything (link down).
    down: AtomicBool,
    /// Bytes forwarded from the sender to the receiver.
    forwarded: AtomicU64,
}

/// A UDP forwarder between a sender and a receiver with adjustable loss,
/// delay and outages; [`LossyLink::change_network`] makes the receiver see
/// the sender at a new address, like after switching from Wi-Fi to mobile data.
struct LossyLink {
    /// Address the sender talks to.
    addr: SocketAddr,
    /// Conditions shared with the forwarding tasks.
    conditions: Arc<Conditions>,
    /// Socket towards the receiver; replaced by `change_network`.
    upstream: Arc<Mutex<Arc<UdpSocket>>>,
    /// Address of the receiving node.
    receiver: SocketAddr,
    /// Address of the sender, learned from its first packet.
    client: Arc<Mutex<Option<SocketAddr>>>,
    /// Socket the sender talks to.
    front: Arc<UdpSocket>,
}

/// Receives the next datagram; `None` when the socket is unusable.
///
/// Windows reports an ICMP "port unreachable" for an earlier send as a
/// `ConnectionReset` error on the next receive (e.g. after a network change
/// closed the old port); that says nothing about this socket, so it is skipped.
async fn receive(socket: &UdpSocket, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
    loop {
        match socket.recv_from(buf).await {
            Ok(received) => return Some(received),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) => {}
            Err(_) => return None,
        }
    }
}

/// Deterministic pseudo-random numbers (xorshift), so a failing run repeats.
struct Dice(u64);

impl Dice {
    fn roll_permille(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 1000) as u32
    }
}

impl LossyLink {
    async fn start(receiver: SocketAddr) -> Result<Self> {
        let front = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        let link = Self {
            addr: front.local_addr()?,
            conditions: Arc::default(),
            upstream: Arc::new(Mutex::new(Arc::new(UdpSocket::bind("127.0.0.1:0").await?))),
            receiver,
            client: Arc::default(),
            front,
        };
        link.spawn_forward_to_receiver();
        link.spawn_forward_to_sender(link.upstream.lock().unwrap().clone());
        Ok(link)
    }

    /// Whether a packet passes, decided by the current conditions.
    fn passes(conditions: &Conditions, dice: &mut Dice) -> bool {
        !conditions.down.load(Ordering::Relaxed)
            && dice.roll_permille() >= conditions.loss_permille.load(Ordering::Relaxed)
    }

    /// Sends `data` to `to` from `socket` after the configured delay.
    fn deliver(conditions: &Conditions, socket: Arc<UdpSocket>, data: Vec<u8>, to: SocketAddr) {
        let delay = Duration::from_millis(conditions.delay_ms.load(Ordering::Relaxed));
        tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let _ = socket.send_to(&data, to).await;
        });
    }

    fn spawn_forward_to_receiver(&self) {
        let (front, upstream, client) = (
            self.front.clone(),
            self.upstream.clone(),
            self.client.clone(),
        );
        let (conditions, receiver) = (self.conditions.clone(), self.receiver);
        tokio::spawn(async move {
            let mut dice = Dice(0x5eed_1234);
            let mut buf = vec![0u8; 65536];
            while let Some((len, from)) = receive(&front, &mut buf).await {
                *client.lock().unwrap() = Some(from);
                if Self::passes(&conditions, &mut dice) {
                    conditions
                        .forwarded
                        .fetch_add(len as u64, Ordering::Relaxed);
                    let socket = upstream.lock().unwrap().clone();
                    Self::deliver(&conditions, socket, buf[..len].to_vec(), receiver);
                }
            }
        });
    }

    fn spawn_forward_to_sender(&self, upstream: Arc<UdpSocket>) {
        let (front, client, conditions) = (
            self.front.clone(),
            self.client.clone(),
            self.conditions.clone(),
        );
        let current = self.upstream.clone();
        tokio::spawn(async move {
            let mut dice = Dice(0x00c0_ffee);
            let mut buf = vec![0u8; 65536];
            while let Some((len, _)) = receive(&upstream, &mut buf).await {
                // Packets to an old address are lost, as after a real network change.
                if !Arc::ptr_eq(&upstream, &current.lock().unwrap()) {
                    break;
                }
                let to = *client.lock().unwrap();
                if let Some(to) = to
                    && Self::passes(&conditions, &mut dice)
                {
                    Self::deliver(&conditions, front.clone(), buf[..len].to_vec(), to);
                }
            }
        });
    }

    /// The receiver sees the sender at a new address from now on.
    async fn change_network(&self) -> Result<()> {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        *self.upstream.lock().unwrap() = socket.clone();
        self.spawn_forward_to_sender(socket);
        Ok(())
    }

    fn set(&self, loss_permille: u32, delay_ms: u64) {
        self.conditions
            .loss_permille
            .store(loss_permille, Ordering::Relaxed);
        self.conditions.delay_ms.store(delay_ms, Ordering::Relaxed);
    }

    fn set_down(&self, down: bool) {
        self.conditions.down.store(down, Ordering::Relaxed);
    }

    /// The receiver's address as the sender knows it: only through this link.
    fn target(&self, id: EndpointId) -> EndpointAddr {
        EndpointAddr::from_parts(id, [TransportAddr::Ip(self.addr)])
    }
}

/// Selects only paths that do not lead straight to the other node's sockets,
/// i.e. the path through the [`LossyLink`].
#[derive(Debug, Default)]
struct ThroughLinkOnly {
    /// UDP ports of the other node; paths to them are never selected.
    direct_ports: Mutex<HashSet<u16>>,
}

impl PathSelector for ThroughLinkOnly {
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        let forbidden = self.direct_ports.lock().unwrap();
        let allowed = |path: &FourTuple| match path {
            FourTuple::Ip { remote, .. } => !forbidden.contains(&remote.port()),
            _ => false,
        };
        let mut selection = PathSelection::none();
        if ctx.current().is_some_and(allowed) {
            return selection; // keep it
        }
        if let Some(path) = ctx.paths().find(|p| allowed(p.network_path())) {
            selection.set(&path);
        }
        selection
    }
}

/// A local-mode node that sends only on paths `selector` allows.
async fn node(selector: Arc<ThroughLinkOnly>) -> Result<NodeEngine> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(SecretKey::generate())
        .alpns(vec![ALPN.to_vec()])
        .path_selector(selector)
        // The internet mode's settings: lossy, distant links are what it is for.
        .transport_config(ferry::node::engine::transport_config(NetworkMode::Internet))
        .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))?
        .bind()
        .await?;
    Ok(NodeEngine::from_endpoint(endpoint, NetworkMode::Local))
}

/// UDP ports `engine` listens on.
fn ports(engine: &NodeEngine) -> HashSet<u16> {
    engine
        .endpoint()
        .bound_sockets()
        .iter()
        .map(SocketAddr::port)
        .collect()
}

/// Accepts everything into `dir` and counts the questions it was asked.
struct AcceptAll {
    dir: PathBuf,
    questions: AtomicUsize,
}

impl TransferHandler for AcceptAll {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        self.questions.fetch_add(1, Ordering::Relaxed);
        OfferDecision::Accept {
            dest_dir: self.dir.clone(),
        }
    }

    async fn decide_batch(&self, _: EndpointId, _: &BatchOffer) -> Option<OfferDecision> {
        self.questions.fetch_add(1, Ordering::Relaxed);
        Some(OfferDecision::Accept {
            dest_dir: self.dir.clone(),
        })
    }

    fn on_event(&self, _: TransferEvent) {}
}

/// Receiver, sender and a lossy link between them.
struct Setup {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    src: PathBuf,
    dst: PathBuf,
    handler: Arc<AcceptAll>,
    receiver: NodeEngine,
    sender: NodeEngine,
    link: LossyLink,
}

impl Setup {
    async fn new() -> Result<Self> {
        let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
        let handler = Arc::new(AcceptAll {
            dir: dst.path().to_path_buf(),
            questions: AtomicUsize::new(0),
        });
        let (to_receiver, to_sender) = (
            Arc::<ThroughLinkOnly>::default(),
            Arc::<ThroughLinkOnly>::default(),
        );
        let receiver = node(to_sender.clone()).await?;
        let sender = node(to_receiver.clone()).await?;
        *to_receiver.direct_ports.lock().unwrap() = ports(&receiver);
        *to_sender.direct_ports.lock().unwrap() = ports(&sender);
        let receiver_ip = receiver
            .endpoint()
            .bound_sockets()
            .into_iter()
            .find(SocketAddr::is_ipv4)
            .context("receiver has no IPv4 socket")?;
        tokio::spawn(service::serve(receiver.clone(), handler.clone()));
        Ok(Self {
            src: src.path().to_path_buf(),
            dst: dst.path().to_path_buf(),
            _dirs: (src, dst),
            handler,
            link: LossyLink::start(receiver_ip).await?,
            receiver,
            sender,
        })
    }

    /// Writes a file of `size` bytes to send; returns its path and content.
    fn file(&self, name: &str, size: usize) -> Result<(PathBuf, Vec<u8>)> {
        let data: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
        let path = self.src.join(name);
        std::fs::write(&path, &data)?;
        Ok((path, data))
    }

    /// Sends `files` through the link, recording the progress of every event.
    /// A failure reports the final status of every file.
    async fn send(
        &self,
        files: &[PathBuf],
        on_progress: impl Fn(u64) + Send + Sync,
    ) -> Result<Vec<TransferEvent>> {
        let events = Mutex::new(Vec::new());
        let result = service::send_files(
            &self.sender,
            self.link.target(self.receiver.node_id()),
            files,
            &|e| {
                if e.status == TransferStatus::Running {
                    on_progress(e.done);
                }
                events.lock().unwrap().push(e);
            },
        )
        .await;
        let events = events.into_inner().unwrap();
        result.with_context(|| format!("final statuses: {:?}", final_statuses(&events)))?;
        Ok(events)
    }

    fn received(&self, name: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.dst.join(name))?)
    }
}

/// Runs `test` with a time limit long enough for the slow scenarios.
async fn within<T>(
    limit: Duration,
    test: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(limit, test)
        .await
        .context("test timed out")?
}

/// The final status of every transfer in `events`.
fn final_statuses(events: &[TransferEvent]) -> HashMap<String, TransferStatus> {
    events
        .iter()
        .filter(|e| e.status.is_final())
        .map(|e| (e.file_name.clone(), e.status.clone()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_survives_packet_loss_and_delay() -> Result<()> {
    within(Duration::from_secs(120), async {
        let setup = Setup::new().await?;
        setup.link.set(30, 20); // 3 % loss, 20 ms each way
        let (path, data) = setup.file("lossy.bin", 3 * 1024 * 1024)?;
        let started = Instant::now();
        let events = setup.send(&[path], |_| {}).await?;
        assert_eq!(
            setup.received("lossy.bin")?,
            data,
            "verified despite losses"
        );
        assert!(matches!(
            final_statuses(&events)["lossy.bin"],
            TransferStatus::Completed { .. }
        ));
        let through_link = setup.link.conditions.forwarded.load(Ordering::Relaxed);
        assert!(
            through_link >= data.len() as u64,
            "the data went through the lossy link"
        );
        eprintln!(
            "3 MiB with 3 % loss and 40 ms RTT in {:?}",
            started.elapsed()
        );
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "timing-sensitive on slow Windows runners; covered on Linux, where the same QUIC code runs"
)]
async fn transfer_survives_a_network_change() -> Result<()> {
    within(Duration::from_secs(120), async {
        let setup = Setup::new().await?;
        setup.link.set(0, 5);
        let (path, data) = setup.file("roaming.bin", 12 * 1024 * 1024)?;
        let changed = AtomicBool::new(false);
        let link = &setup.link;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let files = [path];
        let send = setup.send(&files, |done| {
            if done > 3 * 1024 * 1024 && !changed.swap(true, Ordering::Relaxed) {
                let _ = tx.send(());
            }
        });
        let switch = async {
            if rx.recv().await.is_some() {
                link.change_network().await.unwrap();
            }
        };
        let (events, ()) = tokio::join!(send, switch);
        let events = events?;
        assert!(
            changed.load(Ordering::Relaxed),
            "the network changed mid-transfer"
        );
        assert_eq!(setup.received("roaming.bin")?, data);
        assert!(matches!(
            final_statuses(&events)["roaming.bin"],
            TransferStatus::Completed { .. }
        ));
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "timing-sensitive on slow Windows runners; covered on Linux, where the same QUIC code runs"
)]
async fn transfer_survives_a_short_outage() -> Result<()> {
    within(Duration::from_secs(120), async {
        let setup = Setup::new().await?;
        setup.link.set(0, 5);
        let (path, data) = setup.file("outage.bin", 12 * 1024 * 1024)?;
        let fired = AtomicBool::new(false);
        let link = &setup.link;
        let files = [path];
        let send = setup.send(&files, |done| {
            if done > 2 * 1024 * 1024 && !fired.swap(true, Ordering::Relaxed) {
                link.set_down(true);
            }
        });
        let restore = async {
            while !fired.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await; // well below the idle timeout
            link.set_down(false);
        };
        let (events, ()) = tokio::join!(send, restore);
        let events = events?;
        assert_eq!(setup.received("outage.bin")?, data);
        assert!(matches!(
            final_statuses(&events)["outage.bin"],
            TransferStatus::Completed { .. }
        ));
        Ok(())
    })
    .await
}

/// The link goes away for longer than the QUIC idle timeout, like a phone
/// in deep sleep: the connection breaks, the sender reconnects by itself, the
/// receiver resumes the partial file, and the user is not asked again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "waits for the 30 s QUIC idle timeout; run with --include-ignored"]
async fn transfer_resumes_after_the_connection_breaks() -> Result<()> {
    within(Duration::from_secs(240), async {
        let setup = Setup::new().await?;
        setup.link.set(0, 5);
        // Large enough that the outage hits it even with 8 MiB in flight.
        let (first, first_data) = setup.file("first.bin", 32 * 1024 * 1024)?;
        let (second, second_data) = setup.file("second.bin", 3 * 1024 * 1024)?;
        let fired = AtomicBool::new(false);
        let link = &setup.link;
        let files = [first, second];
        let send = setup.send(&files, |done| {
            if done > 2 * 1024 * 1024 && !fired.swap(true, Ordering::Relaxed) {
                link.set_down(true);
            }
        });
        let restore = async {
            while !fired.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tokio::time::sleep(Duration::from_secs(36)).await;
            link.set_down(false);
        };
        let (events, ()) = tokio::join!(send, restore);
        let events = events?;

        assert!(
            events
                .iter()
                .any(|e| matches!(e.status, TransferStatus::Retrying { .. })),
            "the sender reported reconnecting"
        );
        assert_eq!(
            setup.received("first.bin")?,
            first_data,
            "resumed and verified"
        );
        assert_eq!(setup.received("second.bin")?, second_data);
        let finals = final_statuses(&events);
        assert!(matches!(
            finals["first.bin"],
            TransferStatus::Completed { .. }
        ));
        assert!(matches!(
            finals["second.bin"],
            TransferStatus::Completed { .. }
        ));
        assert_eq!(
            setup.handler.questions.load(Ordering::Relaxed),
            1,
            "only the batch was asked about, not the resumed files"
        );
        Ok(())
    })
    .await
}
