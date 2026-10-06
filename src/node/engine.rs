use anyhow::{Context, Result};
use iroh_net::NodeAddr;
use iroh_net::endpoint::{Connection, Endpoint};
use iroh_net::key::SecretKey;
use tracing::{debug, info};

pub const ALPN_RQS: &[u8] = b"rqs/v0.1";

pub struct NodeEngine {
    endpoint: Endpoint,
}

impl NodeEngine {
    pub async fn new(secret_key: SecretKey) -> Result<Self> {
        let endpoint = Endpoint::builder()
            .secret_key(secret_key)
            .alpns(vec![ALPN_RQS.to_vec()])
            .bind()
            .await
            .context("Nie udało się utworzyć Endpoint")?;

        info!("Węzeł P2P uruchomiony. Node ID: {}", endpoint.node_id());
        Ok(Self { endpoint })
    }

    pub async fn node_addr(&self) -> Result<NodeAddr> {
        self.endpoint
            .node_addr()
            .await
            .context("Nie udało się pobrać NodeAddr")
    }

    pub async fn accept(&self) -> Result<Connection> {
        debug!("Oczekiwanie na połączenie QUIC...");
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("Endpoint zamknięty"))?;

        let conn = incoming.await.context("Błąd nawiązywania połączenia")?;
        info!("Połączenie przychodzące od: {}", conn.remote_address());
        Ok(conn)
    }

    pub async fn connect(&self, target: NodeAddr) -> Result<Connection> {
        debug!("Łączenie z węzłem...");
        let conn = self
            .endpoint
            .connect(target, ALPN_RQS)
            .await
            .context("Błąd połączenia")?;

        info!("Połączono pomyślnie!");
        Ok(conn)
    }
}
