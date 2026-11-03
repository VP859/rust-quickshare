//! LocalSend compatibility mode: receive files from LocalSend apps.
//!
//! [LocalSend](https://localsend.org) runs on iOS, macOS, Android, Windows and
//! Linux. With this mode on, Ferry speaks LocalSend protocol v2 as a
//! **receiver**: it shows up in the LocalSend app's list of nearby devices
//! and accepts files sent from it, so an iPhone or a Mac can send to Ferry
//! without a Ferry app.
//!
//! * [`LocalSend::start`] runs an HTTPS server on TCP port [`PORT`] (53317)
//!   with a self-signed certificate kept in the configuration folder
//!   ([`tls`]), and announces this device on the multicast group
//!   [`MULTICAST_GROUP`]:53317 ([`multicast`]).
//! * An offer (`prepare-upload`) goes to
//!   [`TransferHandler::decide_localsend`];
//!   the files are received only after the user accepted, into the folder the
//!   interface chose, with the same file-name sanitizing, free-space check,
//!   risky-file warning and Mark-of-the-Web as Ferry transfers.
//!
//! **Security.** This is a compatibility mode with LocalSend's security model,
//! not Ferry's: there is no pairing and the sender is not authenticated – its
//! name is whatever it announces and anyone on the local network can offer
//! files. Data is encrypted with TLS, but a sender does not authenticate
//! itself and Ferry cannot verify who is sending. Therefore the mode is
//! off by default, offers are always asked about (never Auto-Accept) and
//! labelled as coming from LocalSend, there is one session at a time, and an
//! address whose offer was declined must wait before offering again. The
//! Ferry identity (Node ID) is never shown to LocalSend devices. Sending to
//! LocalSend devices is not supported.

pub mod dto;
pub mod multicast;
mod server;
pub mod tls;

use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::node::NodeEngine;
use crate::transfer::risk::Risk;
use crate::transfer::service::TransferHandler;

/// Port of the LocalSend server and of its multicast announcements.
pub const PORT: u16 = 53317;
/// Multicast group of LocalSend announcements.
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);
/// Protocol version announced to LocalSend devices.
pub const PROTOCOL_VERSION: &str = "2.1";
/// Prefix of [`TransferEvent::peer`](crate::transfer::service::TransferEvent::peer)
/// for transfers from LocalSend apps (`localsend:192.168.1.20`).
pub const PEER_PREFIX: &str = "localsend:";

/// A device that sent a LocalSend offer, as it describes itself (not verified).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSendSender {
    /// The name it announces.
    pub alias: String,
    /// The device model it announces (`iPhone`, `Samsung`), if any.
    pub model: Option<String>,
    /// The address the offer came from.
    pub address: IpAddr,
}

impl LocalSendSender {
    /// `alias (model)`, for display.
    pub fn label(&self) -> String {
        match &self.model {
            Some(model) => format!("{} ({model})", self.alias),
            None => self.alias.clone(),
        }
    }

    /// Value of [`TransferEvent::peer`](crate::transfer::service::TransferEvent::peer)
    /// for transfers from this device.
    pub fn peer_id(&self) -> String {
        format!("{PEER_PREFIX}{}", self.address)
    }
}

/// Files offered by a LocalSend app, as passed to
/// [`TransferHandler::decide_localsend`].
#[derive(Debug, Clone)]
pub struct LocalSendOffer {
    /// ID of this offer (the upload session).
    pub id: String,
    /// Who offers the files (not verified).
    pub sender: LocalSendSender,
    /// Number of files.
    pub count: u32,
    /// Their total size in bytes.
    pub total_size: u64,
    /// Names of the first files (with folders, `Photos/a.jpg`), at most
    /// [`MAX_BATCH_NAMES`](crate::protocol::messages::MAX_BATCH_NAMES).
    pub names: Vec<String>,
    /// The most serious [`Risk`] among **all** offered files, not only the
    /// listed ones.
    pub risk: Option<Risk>,
}

/// How to run the LocalSend compatibility mode.
#[derive(Debug, Clone)]
pub struct LocalSendOptions {
    /// Address of the HTTPS server: `0.0.0.0` (tests: `127.0.0.1`).
    pub bind: IpAddr,
    /// Port of the HTTPS server: [`PORT`] (tests: `0` for a free one).
    pub port: u16,
    /// Announce on the multicast group and answer announcements. Off in hidden
    /// mode: the server still answers devices that know the address.
    pub multicast: bool,
    /// Folder of the certificate; `None` is the configuration folder.
    pub cert_dir: Option<PathBuf>,
}

impl LocalSendOptions {
    /// Options for a device with `config`: the default port, and no
    /// announcements in hidden mode.
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            multicast: !config.hidden,
            ..Self::default()
        }
    }
}

impl LocalSendOffer {
    /// The listed names joined with `, `, with `…` when there are more files.
    pub fn names_preview(&self) -> String {
        let mut names = self.names.join(", ");
        if self.names.len() < self.count as usize {
            names.push_str(", …");
        }
        names
    }
}

impl Default for LocalSendOptions {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: PORT,
            multicast: true,
            cert_dir: None,
        }
    }
}

/// The running compatibility mode; stops when dropped.
pub struct LocalSend {
    /// Address the HTTPS server listens on.
    addr: SocketAddr,
    /// Certificate fingerprint LocalSend devices know this device by.
    fingerprint: String,
    /// Stops the server and the announcements.
    stop: CancellationToken,
    /// Server and multicast tasks.
    tasks: Vec<JoinHandle<()>>,
}

impl LocalSend {
    /// Starts the HTTPS server and, with [`LocalSendOptions::multicast`], the
    /// announcements. Offers go to `handler`.
    ///
    /// # Errors
    ///
    /// Fails when the certificate cannot be loaded or created, or the port is
    /// taken (e.g. by the LocalSend app on the same computer).
    pub async fn start<H: TransferHandler>(
        engine: &NodeEngine,
        options: LocalSendOptions,
        handler: Arc<H>,
    ) -> Result<Self> {
        let cert_dir = match options.cert_dir.clone() {
            Some(dir) => dir,
            None => crate::config::paths::config_dir()?,
        };
        let identity = tls::Identity::load_or_create(&cert_dir)?;
        let fingerprint = identity.fingerprint();
        let listener = tokio::net::TcpListener::bind((options.bind, options.port))
            .await
            .with_context(|| {
                rust_i18n::t!("localsend.port_taken", port = options.port).into_owned()
            })?;
        let addr = listener.local_addr()?;
        let stop = CancellationToken::new();
        let device_type = if cfg!(target_os = "android") {
            "mobile"
        } else {
            "desktop"
        };
        let info = server::OwnInfo {
            fingerprint: fingerprint.clone(),
            device_type,
            port: addr.port(),
        };
        let server = Arc::new(server::Server::new(handler.clone(), info.clone()));
        let mut tasks = vec![tokio::spawn(server::run(
            listener,
            identity.acceptor()?,
            server,
            stop.clone(),
        ))];
        if options.multicast {
            let announcer = multicast::Announcer::new(info, handler);
            match multicast::bind(PORT) {
                Ok(socket) => tasks.push(tokio::spawn(multicast::run(
                    engine.clone(),
                    socket,
                    announcer,
                    stop.clone(),
                ))),
                Err(e) => tracing::warn!("LocalSend multicast unavailable: {e}"),
            }
        }
        tracing::info!("LocalSend compatibility mode on {addr} (fingerprint {fingerprint})");
        Ok(Self {
            addr,
            fingerprint,
            stop,
            tasks,
        })
    }

    /// Port of the HTTPS server.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Fingerprint of the certificate (uppercase hex SHA-256), shown by
    /// LocalSend apps as the device's identity.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Stops the server and the announcements; transfers in progress fail.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Waits until everything has stopped (after [`LocalSend::stop`]).
    pub async fn stopped(mut self) {
        self.stop.cancel();
        for task in std::mem::take(&mut self.tasks) {
            let _ = task.await;
        }
    }
}

impl Drop for LocalSend {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
