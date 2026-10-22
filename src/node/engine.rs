use anyhow::{Context, Result};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use std::time::Duration;
use tracing::{debug, info};

use super::NodeTicket;

/// ALPN protokołu RQS. Zmiana formatu ramek wymaga zmiany wersji.
pub const ALPN_RQS: &[u8] = b"rqs/0.2";

/// Czas oczekiwania na połączenie z relay przy generowaniu TICKETU.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Cienka warstwa nad `iroh::Endpoint` (QUIC + hole punching + relay).
/// Tani do klonowania – wszystkie kopie współdzielą ten sam endpoint.
#[derive(Clone, Debug)]
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

    /// Węzeł ze stałą tożsamością z katalogu konfiguracji.
    pub async fn with_persistent_identity() -> Result<Self> {
        Self::new(super::load_or_generate_default_key()?).await
    }

    /// Adres węzła; czeka chwilę na połączenie z relay, aby TICKET zawierał jego URL.
    pub async fn node_addr(&self) -> Result<EndpointAddr> {
        let _ = tokio::time::timeout(ONLINE_TIMEOUT, self.endpoint.online()).await;
        Ok(self.endpoint.addr())
    }

    pub async fn ticket(&self) -> Result<NodeTicket> {
        Ok(NodeTicket::new(self.node_addr().await?))
    }

    /// Czeka na kolejne połączenie przychodzące. `Ok(None)` – endpoint zamknięty.
    pub async fn accept(&self) -> Result<Option<Connection>> {
        debug!("Oczekiwanie na przychodzące połączenie QUIC...");
        let Some(incoming) = self.endpoint.accept().await else {
            return Ok(None);
        };

        let conn = incoming
            .await
            .context("Błąd nawiązywania połączenia wejściowego")?;
        info!("Nawiązano połączenie wejściowe od: {}", conn.remote_id());
        Ok(Some(conn))
    }

    pub async fn connect(&self, target: impl Into<EndpointAddr>) -> Result<Connection> {
        let target = target.into();
        debug!("Nawiązywanie połączenia P2P z {}...", target.id);
        let conn = self
            .endpoint
            .connect(target, ALPN_RQS)
            .await
            .context("Nie udało się połączyć z podanym węzłem")?;

        info!("Połączono z węzłem {}", conn.remote_id());
        Ok(conn)
    }

    /// Sprawdza osiągalność węzła (nawiązanie i natychmiastowe zamknięcie połączenia).
    pub async fn ping(&self, target: impl Into<EndpointAddr>, timeout: Duration) -> bool {
        match tokio::time::timeout(timeout, self.connect(target)).await {
            Ok(Ok(conn)) => {
                conn.close(0u32.into(), b"ping");
                true
            }
            _ => false,
        }
    }

    pub fn node_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}
