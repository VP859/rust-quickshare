pub mod hash;
pub mod receiver;
pub mod security;
pub mod sender;
pub mod service;

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream};
use std::time::Duration;
use tracing::debug;

use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::Hello;

/// Wywoływane z liczbą bajtów już przesłanych/zapisanych.
pub type ProgressFn<'a> = &'a (dyn Fn(u64) + Send + Sync);

/// Czas oczekiwania na potwierdzenie odbioru ostatnich ramek przed zamknięciem strumienia.
const LINGER: Duration = Duration::from_secs(5);

/// Dwukierunkowy strumień kontrolny jednego transferu
/// (Hello → Offer → Decision → … → TransferResult).
pub struct Channel {
    pub send: SendStream,
    pub recv: RecvStream,
}

impl Channel {
    /// Otwiera strumień kontrolny po stronie łączącej się i wysyła `Hello`.
    pub async fn open(conn: &Connection, hello: &Hello) -> Result<Self> {
        let (mut send, recv) = conn
            .open_bi()
            .await
            .context("Nie udało się otworzyć strumienia kontrolnego")?;
        write_msg(&mut send, hello).await?;
        Ok(Self { send, recv })
    }

    /// Przyjmuje kolejny strumień kontrolny. `Ok(None)` – połączenie zakończone przez drugą stronę.
    pub async fn accept(conn: &Connection) -> Result<Option<(Self, Hello)>> {
        let (send, mut recv) = match conn.accept_bi().await {
            Ok(pair) => pair,
            Err(
                ConnectionError::ApplicationClosed(_)
                | ConnectionError::ConnectionClosed(_)
                | ConnectionError::LocallyClosed
                | ConnectionError::TimedOut,
            ) => return Ok(None),
            Err(e) => return Err(e).context("Błąd przyjmowania strumienia kontrolnego"),
        };
        let hello: Hello = read_msg(&mut recv).await?;
        Ok(Some((Self { send, recv }, hello)))
    }

    /// Kończy strumień i czeka (ograniczony czas), aż druga strona potwierdzi odbiór danych.
    pub async fn finish(mut self) {
        if self.send.finish().is_ok()
            && tokio::time::timeout(LINGER, self.send.stopped())
                .await
                .is_err()
        {
            debug!("Upłynął czas oczekiwania na potwierdzenie zamknięcia strumienia");
        }
    }
}

pub(crate) fn short_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
