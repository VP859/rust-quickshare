use anyhow::{Context, Result};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, SecretKey};
use tracing::{debug, info};

pub const ALPN_RQS: &[u8] = b"rqs/v0.1";

pub struct NodeEngine {
    endpoint: Endpoint,
}

impl NodeEngine {
    pub async fn new(secret_key: SecretKey) -> Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key)
            .alpns(vec![ALPN_RQS.to_vec()])
            .bind()
            .await
            .context("Nie udało się powiązać Iroh Endpoint")?;

        info!("Węzeł P2P uruchomiony. Endpoint ID: {}", endpoint.id());
        Ok(Self { endpoint })
    }

    pub async fn node_addr(&self) -> Result<EndpointAddr> {
        Ok(self.endpoint.addr())
    }

    pub async fn accept(&self) -> Result<Connection> {
        debug!("Oczekiwanie na przychodzące połączenie QUIC...");
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("Endpoint został zamknięty"))?;

        let conn = incoming
            .await
            .context("Błąd nawiązywania połączenia wejściowego")?;
        info!("Nawiązano połączenie wejściowe od: {}", conn.remote_id());
        Ok(conn)
    }

    pub async fn connect(&self, target: EndpointAddr) -> Result<Connection> {
        debug!("Nawiązywanie połączenia P2P z węzłem docelowym...");
        let conn = self
            .endpoint
            .connect(target, ALPN_RQS)
            .await
            .context("Nie udało się połączyć z podanym węzłem")?;

        info!("Połączono pomyślnie z podanym EndpointAddr!");
        Ok(conn)
    }

    pub fn node_id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    pub async fn close(self) -> Result<()> {
        self.endpoint.close().await;
        Ok(())
    }
}
