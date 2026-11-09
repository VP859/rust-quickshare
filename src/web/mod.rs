//! Sharing through a web browser: send files to, and receive files from, a
//! phone or computer that has no Ferry, on the same local network.
//!
//! [`WebShare::start`] runs a small HTTP server and returns a link with a
//! **one-time token** (`http://192.168.1.10:47835/3f9c…/`), shown as text and
//! as a QR code. Opening it in any browser shows the files shared in this
//! session for download and, with [`WebShareOptions::receive`], a button to
//! send files to this device.
//!
//! **Security.** Browsers do not accept self-signed certificates without a
//! frightening warning, so the link uses plain HTTP. What has content – the
//! file list, the files, the names and contents of files from the browser – is
//! encrypted end to end with a key that is only in the link after `#`
//! ([`seal`]), so someone who watches the network sees the token, sizes and
//! timing, not the files. Someone who can *change* the traffic could change
//! the page and make it give the key away; against that only a trusted network
//! helps. Against the other risks:
//!
//! * the token has 128 random bits and is the only way in: every other path
//!   answers `404`, and nothing tells that a Ferry is listening;
//! * the first device that uses the token owns the session; requests with it
//!   from any other address are refused, so a token seen on the network or in
//!   a photo of the screen is of no use once the link was opened;
//! * the session ends after [`WebShareOptions::lifetime`] (30 minutes by
//!   default) or when it is stopped, and the server is gone with it;
//! * files from the browser are saved only after the user accepted them in
//!   Ferry ([`TransferHandler::decide_browser`]), with the same file-name
//!   sanitizing, size and free-space checks, risky-file warnings and
//!   Mark-of-the-Web as all other transfers; a declined browser must wait;
//! * only the files chosen for the session can be downloaded;
//! * the page is served with a strict Content-Security-Policy, downloads are
//!   decrypted and saved by the page (never shown), and no response may be
//!   cached or framed.

mod body;
pub mod seal;
mod server;

use anyhow::{Context, Result};
use rust_i18n::t;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::node::NodeEngine;
use crate::transfer::risk::Risk;
use crate::transfer::service::TransferHandler;

/// Preferred TCP port of the web server; a random one is used when it is taken.
pub const WEB_PORT: u16 = 47835;
/// How long a session lasts by default.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(30 * 60);
/// Prefix of [`TransferEvent::peer`](crate::transfer::service::TransferEvent::peer)
/// for transfers with a browser (`browser:192.168.1.20`).
pub const PEER_PREFIX: &str = "browser:";

/// Files a browser wants to send, passed to
/// [`TransferHandler::decide_browser`].
#[derive(Debug, Clone)]
pub struct BrowserOffer {
    /// ID of this offer.
    pub id: String,
    /// Address of the browser.
    pub address: IpAddr,
    /// The browser and system, from its `User-Agent` (`Safari on iPhone`);
    /// not verified.
    pub browser: String,
    /// Number of files.
    pub count: u32,
    /// Their total size in bytes.
    pub total_size: u64,
    /// Names of the first files, at most
    /// [`MAX_BATCH_NAMES`](crate::protocol::messages::MAX_BATCH_NAMES).
    pub names: Vec<String>,
    /// The most serious [`Risk`] among **all** offered files.
    pub risk: Option<Risk>,
}

impl BrowserOffer {
    /// The listed names joined with `, `, with `…` when there are more files.
    pub fn names_preview(&self) -> String {
        let mut names = self.names.join(", ");
        if self.names.len() < self.count as usize {
            names.push_str(", …");
        }
        names
    }

    /// `Web browser (Safari on iPhone) at 192.168.1.20`, for display.
    pub fn label(&self) -> String {
        label(&self.browser, self.address)
    }
}

/// How a browser is shown in transfers and questions.
fn label(browser: &str, address: IpAddr) -> String {
    t!("web.browser_label", browser = browser, address = address).into_owned()
}

/// What a session offers.
#[derive(Debug, Clone)]
pub struct WebShareOptions {
    /// Address of the server: `0.0.0.0` (tests: `127.0.0.1`).
    pub bind: IpAddr,
    /// Port of the server: [`WEB_PORT`], or `0` for a free one.
    pub port: u16,
    /// The session ends after this time; `None` keeps it until stopped.
    pub lifetime: Option<Duration>,
    /// Files the browser can download (regular files only).
    pub files: Vec<PathBuf>,
    /// The browser may send files (each batch is asked about).
    pub receive: bool,
}

impl Default for WebShareOptions {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: WEB_PORT,
            lifetime: Some(DEFAULT_LIFETIME),
            files: Vec::new(),
            receive: true,
        }
    }
}

/// A running browser session; it ends when dropped.
pub struct WebShare {
    /// Links to open, one per local network address.
    urls: Vec<String>,
    /// Address the server listens on.
    addr: SocketAddr,
    /// When the session ends by itself.
    expires_at: Option<Instant>,
    /// Ends the session.
    stop: CancellationToken,
    /// The server; finishes when the session ends.
    task: JoinHandle<()>,
}

impl WebShare {
    /// Starts a session. `engine` helps finding the local addresses on
    /// Android and may be `None` elsewhere.
    ///
    /// # Errors
    ///
    /// Fails when a shared path is not a regular file or no port can be opened.
    pub async fn start<H: TransferHandler>(
        engine: Option<&NodeEngine>,
        options: WebShareOptions,
        handler: Arc<H>,
    ) -> Result<Self> {
        let mut files = Vec::with_capacity(options.files.len());
        for path in &options.files {
            let meta = std::fs::metadata(path)
                .with_context(|| t!("web.not_a_file", path = path.display()).into_owned())?;
            if !meta.is_file() {
                anyhow::bail!(t!("web.not_a_file", path = path.display()));
            }
            files.push(server::SharedFile {
                name: crate::transfer::security::sanitize_filename(
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                )?,
                path: path.clone(),
                size: meta.len(),
            });
        }
        let listener = match tokio::net::TcpListener::bind((options.bind, options.port)).await {
            Ok(listener) => listener,
            Err(e) if options.port != 0 => {
                tracing::warn!(
                    "Web port {} unavailable ({e}), using a random one",
                    options.port
                );
                tokio::net::TcpListener::bind((options.bind, 0)).await?
            }
            Err(e) => return Err(e.into()),
        };
        let addr = listener.local_addr()?;
        let token = new_token()?;
        let key = seal::SessionKey::generate()?;
        let urls = urls(engine, options.bind, addr.port(), &token)
            .into_iter()
            .map(|url| format!("{url}#{}", key.fragment()))
            .collect();
        let expires_at = options.lifetime.map(|l| Instant::now() + l);
        let stop = CancellationToken::new();
        let server = Arc::new(server::WebServer::new(
            handler,
            token,
            key,
            files,
            options.receive,
            expires_at,
        ));
        let ends = stop.clone();
        let task = tokio::spawn(async move {
            let expired = async {
                match options.lifetime {
                    Some(lifetime) => tokio::time::sleep(lifetime).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = crate::httpd::serve(listener, None, server, ends.clone()) => {}
                () = expired => ends.cancel(),
                () = ends.cancelled() => {}
            }
        });
        tracing::info!("Browser session on {addr}");
        Ok(Self {
            urls,
            addr,
            expires_at,
            stop,
            task,
        })
    }

    /// The link to open (on the first local network).
    pub fn url(&self) -> &str {
        self.urls.first().map_or("", String::as_str)
    }

    /// Links for every local network address of this device.
    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    /// Port of the server.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Time left until the session ends by itself.
    pub fn expires_in(&self) -> Option<Duration> {
        self.expires_at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    /// Ends the session now; transfers in progress fail.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Whether the session has ended (expired or stopped).
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Waits until the session ends.
    pub async fn finished(&mut self) {
        let _ = (&mut self.task).await;
    }
}

impl Drop for WebShare {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// A hint with the command that opens TCP `port` (of [`WebShare::port`]) when
/// firewalld blocks it, so other devices could not open the link. Runs
/// external programs: call it from a blocking context (`spawn_blocking`).
pub fn firewall_hint(port: u16) -> Option<String> {
    crate::discovery::blocking_firewall_for(port, "tcp")
        .map(|_| t!("web.firewalld", port = port).into_owned())
}

/// 128 random bits as 32 hexadecimal characters.
fn new_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Links for the local addresses (only loopback when bound to it).
fn urls(engine: Option<&NodeEngine>, bind: IpAddr, port: u16, token: &str) -> Vec<String> {
    let link = |ip: IpAddr| format!("http://{ip}:{port}/{token}/");
    if bind.is_loopback() {
        return vec![link(bind)];
    }
    if !bind.is_unspecified() {
        return vec![link(bind)];
    }
    let mut addresses: Vec<Ipv4Addr> = crate::discovery::local_networks(engine)
        .into_iter()
        .map(|(ip, _)| ip)
        .collect();
    addresses.sort_by_key(|ip| home_network_rank(*ip));
    let mut urls: Vec<String> = addresses
        .into_iter()
        .map(|ip| link(IpAddr::V4(ip)))
        .collect();
    if urls.is_empty() {
        urls.push(link(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }
    urls
}

/// Order of addresses in the links: home routers use 192.168.0.0/16 most,
/// while VPNs and container networks often use 10.0.0.0/8, so the first link
/// (the QR code) is most likely the one a phone on the same Wi-Fi can reach.
fn home_network_rank(ip: Ipv4Addr) -> u8 {
    match ip.octets() {
        [192, 168, ..] => 0,
        [172, 16..=31, ..] => 1,
        [10, ..] => 2,
        _ if ip.is_link_local() => 4,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_different() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn home_networks_come_first() {
        let mut ips: Vec<Ipv4Addr> = ["10.5.0.2", "169.254.1.1", "172.17.0.1", "192.168.1.20"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        ips.sort_by_key(|ip| home_network_rank(*ip));
        assert_eq!(ips[0], Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(ips[3], Ipv4Addr::new(169, 254, 1, 1));
    }

    #[test]
    fn links_contain_the_port_and_the_token() {
        let urls = urls(None, Ipv4Addr::LOCALHOST.into(), 1234, "abc");
        assert_eq!(urls, ["http://127.0.0.1:1234/abc/"]);
    }
}
