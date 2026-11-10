//! The relay modes (`NetworkMode::Relay`, `NetworkMode::RelayOnly`): devices
//! that know only each other's relay URL – no IP address – connect and
//! transfer through a relay server run in the test, and no n0 server is
//! configured; in the relay-only mode never directly, even when they could.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ferry::node::{Network, NetworkMode, NodeEngine, NodeTicket};
use ferry::protocol::messages::TransferOffer;
use ferry::transfer::service::{
    self, OfferDecision, Route, TransferEvent, TransferHandler, TransferStatus,
};
use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey, TransportAddr};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A relay on this computer over plain HTTP (like `iroh-relay --dev`).
async fn relay() -> (Server, RelayUrl) {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new((Ipv4Addr::LOCALHOST, 0)));
    let server = Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    (server, url)
}

async fn node(relay: &RelayUrl) -> NodeEngine {
    node_in(NetworkMode::Relay, relay).await
}

async fn node_in(mode: NetworkMode, relay: &RelayUrl) -> NodeEngine {
    let network = Network {
        mode,
        relays: vec![relay.clone()],
        random_port: false,
    };
    NodeEngine::with_network(SecretKey::generate(), network)
        .await
        .unwrap()
}

struct AcceptAll(PathBuf);

impl TransferHandler for AcceptAll {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Accept {
            dest_dir: self.0.clone(),
        }
    }
    fn on_event(&self, _: TransferEvent) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_reach_each_other_through_their_own_relay() {
    let (_server, url) = relay().await;
    let (sender, receiver) = (node(&url).await, node(&url).await);

    // The address in a TICKET (and the one saved at pairing) names the relay.
    let addr = receiver.node_addr().await.unwrap();
    assert_eq!(
        addr.relay_urls().collect::<Vec<_>>(),
        [&url],
        "only our relay, no n0 relay"
    );
    let ticket = NodeTicket::deserialize_from_str(
        &receiver
            .ticket()
            .await
            .unwrap()
            .serialize_to_string()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(ticket.addr.relay_urls().next(), Some(&url));

    let dst = tempfile::tempdir().unwrap();
    tokio::spawn(service::serve(
        receiver.clone(),
        Arc::new(AcceptAll(dst.path().into())),
    ));

    // Only the relay URL: no IP address the sender could try directly.
    let target = EndpointAddr::from_parts(receiver.node_id(), [TransportAddr::Relay(url.clone())]);
    let src = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(src.path().join("via-relay.bin"), &data).unwrap();
    let finals = std::sync::Mutex::new(Vec::new());
    tokio::time::timeout(
        Duration::from_secs(60),
        service::send_files(&sender, target, &[src.path().join("via-relay.bin")], &|e| {
            if e.status.is_final() {
                finals.lock().unwrap().push(e.status.clone());
            }
        }),
    )
    .await
    .expect("transfer through the relay in time")
    .unwrap();
    let finals = finals.into_inner().unwrap();
    assert!(
        matches!(finals.as_slice(), [TransferStatus::Completed { .. }]),
        "{finals:?}"
    );
    assert_eq!(
        std::fs::read(dst.path().join("via-relay.bin")).unwrap(),
        data
    );
    sender.close().await;
    receiver.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_relay_modes_do_not_start_without_a_relay() {
    for mode in [NetworkMode::Relay, NetworkMode::RelayOnly] {
        let network = Network {
            mode,
            relays: Vec::new(),
            random_port: false,
        };
        assert!(
            NodeEngine::with_network(SecretKey::generate(), network)
                .await
                .is_err(),
            "{mode}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_relay_only_mode_never_connects_directly() {
    let (_server, url) = relay().await;
    // The receiver could be reached directly (relay mode, same computer) …
    let receiver = node(&url).await;
    let sender = node_in(NetworkMode::RelayOnly, &url).await;

    // … the relay-only sender has no IP address to give away.
    let own = sender.node_addr().await.unwrap();
    assert_eq!(own.ip_addrs().count(), 0, "{own:?}");
    assert_eq!(own.relay_urls().collect::<Vec<_>>(), [&url]);
    let target = receiver.node_addr().await.unwrap();
    assert!(target.ip_addrs().next().is_some(), "{target:?}");

    let dst = tempfile::tempdir().unwrap();
    tokio::spawn(service::serve(
        receiver.clone(),
        Arc::new(AcceptAll(dst.path().into())),
    ));
    let src = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(src.path().join("relay-only.bin"), &data).unwrap();
    let routes = std::sync::Mutex::new(Vec::new());
    tokio::time::timeout(
        Duration::from_secs(60),
        service::send_files(
            &sender,
            target,
            &[src.path().join("relay-only.bin")],
            &|e| {
                if let Some(route) = e.route {
                    routes.lock().unwrap().push(route);
                }
            },
        ),
    )
    .await
    .expect("transfer through the relay in time")
    .unwrap();
    let routes = routes.into_inner().unwrap();
    assert!(!routes.is_empty());
    assert!(routes.iter().all(|r| *r == Route::Relay), "{routes:?}");
    assert_eq!(
        std::fs::read(dst.path().join("relay-only.bin")).unwrap(),
        data
    );

    // A device in the local mode, knowing no relay, cannot reach it at all.
    let local = NodeEngine::with_network(SecretKey::generate(), NetworkMode::Local.into())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), local.connect(own))
            .await
            .map_or(true, |r| r.is_err())
    );
    local.close().await;
    sender.close().await;
    receiver.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_local_mode_has_no_relay_in_its_address() {
    let node = NodeEngine::with_network(SecretKey::generate(), NetworkMode::Local.into())
        .await
        .unwrap();
    assert_eq!(node.node_addr().await.unwrap().relay_urls().count(), 0);
    node.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_working_relay_answers_in_time() {
    let (_server, url) = relay().await;
    let took = ferry::node::check_relay(&url).await.unwrap();
    assert!(took < ferry::node::RELAY_CHECK_TIMEOUT, "{took:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_does_not_work_says_why() {
    let check = async |url: &str| {
        let url: RelayUrl = url.parse().unwrap();
        format!("{:#}", ferry::node::check_relay(&url).await.unwrap_err())
    };
    // Nothing listens on a port that was just free.
    let port = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let error = check(&format!("http://127.0.0.1:{port}")).await;
    assert!(
        error.starts_with("Cannot connect to 127.0.0.1 on port"),
        "{error}"
    );

    // A relay without TLS, addressed with https://.
    let (_server, url) = relay().await;
    let error = check(&url.as_str().replace("http://", "https://")).await;
    assert!(error.contains("(TLS)"), "{error}");

    // A web server that is not a relay.
    let web = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = web.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut socket, _)) = web.accept().await {
            let mut buffer = [0; 1024];
            let _ = socket.read(&mut buffer).await;
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .await;
        }
    });
    let error = check(&format!("http://{address}")).await;
    assert!(error.contains("HTTP 404"), "{error}");

    // A name that does not exist (.invalid never resolves).
    let error = check("https://relay.invalid").await;
    assert!(
        error.starts_with("Cannot connect to relay.invalid on port 443"),
        "{error}"
    );
}
