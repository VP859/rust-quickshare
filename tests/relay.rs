//! The relay mode (`NetworkMode::Relay`): devices that know only each other's
//! relay URL – no IP address – connect and transfer through a relay server
//! run in the test, and no n0 server is configured.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ferry::node::{Network, NetworkMode, NodeEngine, NodeTicket};
use ferry::protocol::messages::TransferOffer;
use ferry::transfer::service::{
    self, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
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
    let network = Network {
        mode: NetworkMode::Relay,
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
async fn the_relay_mode_does_not_start_without_a_relay() {
    let network = Network {
        mode: NetworkMode::Relay,
        relays: Vec::new(),
        random_port: false,
    };
    assert!(
        NodeEngine::with_network(SecretKey::generate(), network)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_local_mode_has_no_relay_in_its_address() {
    let node = NodeEngine::with_network(SecretKey::generate(), NetworkMode::Local.into())
        .await
        .unwrap();
    assert_eq!(node.node_addr().await.unwrap().relay_urls().count(), 0);
    node.close().await;
}
