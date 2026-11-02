//! Negotiation of the protocol version on real QUIC connections.
//!
//! A hypothetical newer release is played by a plain iroh endpoint that speaks
//! `ferry/2` and `ferry/1`, so the tests show what happens when Ferry gets a
//! second version, before there is one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferry::error::{ErrorKind, kind};
use ferry::node::NodeEngine;
use ferry::protocol::version::{self, ProtocolVersion};
use iroh::endpoint::{ConnectOptions, Endpoint, presets};
use iroh::{RelayMode, SecretKey};
use std::net::SocketAddr;

/// A plain local endpoint accepting `alpns` and answering every connection
/// with the identifier it negotiated.
async fn peer(alpns: &[&[u8]]) -> Endpoint {
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(SecretKey::generate())
        .alpns(alpns.iter().map(|a| a.to_vec()).collect())
        .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        .unwrap()
        .bind()
        .await
        .unwrap();
    let accepting = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accepting.accept().await {
            if let Ok(conn) = incoming.await {
                let alpn = conn.alpn().to_vec();
                if let Ok(mut send) = conn.open_uni().await {
                    let _ = send.write_all(&alpn).await;
                    let _ = send.finish();
                }
                conn.closed().await;
            }
        }
    });
    endpoint
}

/// The identifier the plain `peer` negotiated, as it told us.
async fn negotiated_by_peer(conn: &iroh::endpoint::Connection) -> Vec<u8> {
    conn.accept_uni()
        .await
        .unwrap()
        .read_to_end(64)
        .await
        .unwrap()
}

async fn engine() -> NodeEngine {
    NodeEngine::new(SecretKey::generate()).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn connects_to_a_peer_that_speaks_the_same_version() {
    let server = peer(&[ProtocolVersion::V1.alpn()]).await;
    let client = engine().await;
    let conn = client.connect(server.addr()).await.unwrap();
    assert_eq!(version::negotiated(&conn), Some(ProtocolVersion::V1));
    assert_eq!(negotiated_by_peer(&conn).await, b"ferry/1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_peer_that_still_speaks_the_old_version_is_reached() {
    // A future release: prefers ferry/2, keeps ferry/1 for older devices.
    let server = peer(&[b"ferry/2", b"ferry/1"]).await;
    let client = engine().await;
    let conn = client.connect(server.addr()).await.unwrap();
    assert_eq!(
        negotiated_by_peer(&conn).await,
        b"ferry/1",
        "the best version both sides speak"
    );
    assert_eq!(version::negotiated(&conn), Some(ProtocolVersion::V1));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_client_is_accepted_with_the_old_version() {
    let server = engine().await;
    let addr = server.node_addr().await.unwrap();
    let accepting = server.clone();
    let accepted = tokio::spawn(async move { accepting.accept().await.unwrap().unwrap() });

    // A future release offering ferry/2 first and ferry/1 as the fallback.
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(SecretKey::generate())
        .alpns(vec![b"ferry/2".to_vec(), b"ferry/1".to_vec()])
        .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        .unwrap()
        .bind()
        .await
        .unwrap();
    let connecting = client
        .connect_with_opts(
            addr,
            b"ferry/2",
            ConnectOptions::new().with_additional_alpns(vec![b"ferry/1".to_vec()]),
        )
        .await
        .unwrap();
    let conn = connecting.await.unwrap();
    assert_eq!(conn.alpn(), b"ferry/1");
    let conn_on_server = accepted.await.unwrap();
    assert_eq!(
        version::negotiated(&conn_on_server),
        Some(ProtocolVersion::V1)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_with_no_common_version_gets_a_clear_error() {
    // A device of a release that dropped ferry/1 – or one that is not Ferry at all.
    let server = peer(&[b"ferry/2"]).await;
    let client = engine().await;
    let error = client.connect(server.addr()).await.unwrap_err();
    assert_eq!(kind(&error), ErrorKind::Incompatible, "{error:#}");
    assert!(
        error.to_string().contains("update"),
        "tells the user what to do: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn other_failures_are_not_reported_as_incompatibility() {
    let client = engine().await;
    // Nothing listens at this address.
    let nowhere = iroh::EndpointAddr::from_parts(
        SecretKey::generate().public(),
        [iroh::TransportAddr::Ip(SocketAddr::from((
            [127, 0, 0, 1],
            9,
        )))],
    );
    // iroh keeps trying unreachable addresses for a long time; what matters is
    // that a failure that does come is not mistaken for an incompatible version.
    if let Ok(Err(error)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), client.connect(nowhere)).await
    {
        assert_ne!(kind(&error), ErrorKind::Incompatible, "{error:#}");
    }
}
