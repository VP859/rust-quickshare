//! LocalSend apps on the local network: what they announce
//! ([`LocalSendDevice`]), the list of those heard recently, and [`scan`], which
//! announces this device so they answer.

use super::http::{Client, empty};
use super::{OwnDevice, own_info};
use crate::localsend::dto::DeviceInfo;
use crate::localsend::{PEER_PREFIX, PORT};
use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, StatusCode};
use rust_i18n::t;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tracing::debug;

/// How long a device stays in [`nearby`] after it was last heard. LocalSend
/// apps announce themselves only when they are opened or refreshed.
const NEARBY_TTL: Duration = Duration::from_secs(15 * 60);

/// Most devices remembered, so a flood of announcements cannot fill memory.
const MAX_NEARBY: usize = 64;

/// A LocalSend app on the local network, as it describes itself (not verified).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LocalSendDevice {
    /// Name it announces.
    pub alias: String,
    /// Device model it announces (`iPhone`, `Samsung`), if any.
    pub model: Option<String>,
    /// `mobile`, `desktop`, … if given.
    pub device_type: Option<String>,
    /// SHA-256 of its certificate (hex) as announced; the connection must
    /// present exactly this certificate.
    pub fingerprint: String,
    /// Address of its server.
    pub address: SocketAddr,
    /// HTTPS (the default) or plain HTTP (when the app's encryption is off).
    pub https: bool,
}

impl LocalSendDevice {
    /// The device described by `info`, which came from `ip`; `None` when it
    /// has no fingerprint (every LocalSend app sends one).
    pub fn from_info(info: &DeviceInfo, ip: IpAddr) -> Option<Self> {
        let fingerprint = info.fingerprint.trim();
        if fingerprint.is_empty() || fingerprint.len() > 128 {
            return None;
        }
        Some(Self {
            alias: info.clean_alias(),
            model: info.clean_model(),
            device_type: info
                .device_type
                .as_deref()
                .map(crate::pairing::clean_device_name)
                .filter(|t| !t.is_empty()),
            fingerprint: fingerprint.to_string(),
            address: SocketAddr::new(ip.to_canonical(), info.port.unwrap_or(PORT)),
            https: info.protocol.as_deref() != Some("http"),
        })
    }

    /// `alias (model)`, for display.
    pub fn label(&self) -> String {
        match &self.model {
            Some(model) => format!("{} ({model})", self.alias),
            None => self.alias.clone(),
        }
    }

    /// Value of [`TransferEvent::peer`](crate::transfer::service::TransferEvent::peer)
    /// for transfers to this device.
    pub fn peer_id(&self) -> String {
        format!("{PEER_PREFIX}{}", self.address.ip())
    }

    /// Asks the LocalSend app at `ip:port` (default [`PORT`]) who it is (as
    /// `me`), for
    /// sending to an address typed by the user. Over HTTPS its certificate is
    /// taken as it is (there is no announcement to compare it with) and
    /// pinned for the rest of the transfer.
    ///
    /// # Errors
    ///
    /// Fails when nothing answers like a LocalSend app there.
    pub async fn probe(ip: IpAddr, port: Option<u16>, me: &OwnDevice) -> Result<Self> {
        let identity = own_info(me).ok().map(|(_, identity)| identity);
        let address = SocketAddr::new(ip.to_canonical(), port.unwrap_or(PORT));
        let mut last_error = None;
        for https in [true, false] {
            let target = Self {
                alias: String::new(),
                model: None,
                device_type: None,
                fingerprint: String::new(),
                address,
                https,
            };
            match Client::new(&target, None, identity.clone())
                .call("GET", "info", empty(), None)
                .await
            {
                Ok((StatusCode::OK, body, seen)) => {
                    let mut info: DeviceInfo = serde_json::from_slice(&body)
                        .with_context(|| t!("localsend.not_localsend", address = address))?;
                    if https {
                        info.fingerprint = seen.unwrap_or_default();
                    }
                    info.port = Some(address.port());
                    info.protocol = Some(if https { "https" } else { "http" }.into());
                    let device = Self::from_info(&info, ip)
                        .with_context(|| t!("localsend.not_localsend", address = address))?;
                    remember(&device);
                    return Ok(device);
                }
                Ok((status, ..)) => {
                    last_error = Some(anyhow::anyhow!("HTTP {status}"));
                }
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("no answer"))
            .context(t!("localsend.not_localsend", address = address)))
    }
}

/// Devices heard recently, by fingerprint.
static NEARBY: LazyLock<Mutex<HashMap<String, (LocalSendDevice, Instant)>>> =
    LazyLock::new(Mutex::default);

/// Counts changes of [`nearby`], for [`changes`].
static CHANGES: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::Sender::new(0));

/// Changes whenever a device is added to [`nearby`] or its description changes.
pub fn changes() -> tokio::sync::watch::Receiver<u64> {
    CHANGES.subscribe()
}

/// Adds or refreshes a device in [`nearby`].
pub(crate) fn remember(device: &LocalSendDevice) {
    let changed = {
        let Ok(mut nearby) = NEARBY.lock() else {
            return;
        };
        nearby.retain(|_, (_, seen)| seen.elapsed() < NEARBY_TTL);
        let key = device.fingerprint.to_ascii_uppercase();
        if nearby.len() >= MAX_NEARBY && !nearby.contains_key(&key) {
            return;
        }
        let changed = nearby.get(&key).is_none_or(|(known, _)| known != device);
        if changed {
            debug!("LocalSend device '{}' at {}", device.alias, device.address);
        }
        nearby.insert(key, (device.clone(), Instant::now()));
        changed
    };
    if changed {
        CHANGES.send_modify(|n| *n += 1);
    }
}

/// Records the device described by a multicast message or a `register`
/// request from `ip`, unless it is this device (`own_fingerprint`).
pub(crate) fn remember_info(info: &DeviceInfo, ip: IpAddr, own_fingerprint: &str) {
    if !info.fingerprint.eq_ignore_ascii_case(own_fingerprint)
        && let Some(device) = LocalSendDevice::from_info(info, ip)
    {
        remember(&device);
    }
}

/// LocalSend apps heard on the local network in the last 15 minutes, sorted by
/// name.
pub fn nearby() -> Vec<LocalSendDevice> {
    let mut list: Vec<_> = NEARBY
        .lock()
        .map(|nearby| {
            nearby
                .values()
                .filter(|(_, seen)| seen.elapsed() < NEARBY_TTL)
                .map(|(device, _)| device.clone())
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|d| (d.alias.to_lowercase(), d.address));
    list
}

/// Finds a device of [`nearby`] by its name (case-insensitive), its IP address
/// or its fingerprint.
pub fn find(query: &str) -> Option<LocalSendDevice> {
    let query = query.trim();
    nearby().into_iter().find(|d| {
        d.alias.eq_ignore_ascii_case(query)
            || d.label().eq_ignore_ascii_case(query)
            || d.address.ip().to_string() == query
            || d.address.to_string() == query
            || d.fingerprint.eq_ignore_ascii_case(query)
    })
}

/// The device of [`nearby`] with exactly this certificate fingerprint.
pub fn find_by_fingerprint(fingerprint: &str) -> Option<LocalSendDevice> {
    nearby()
        .into_iter()
        .find(|d| d.fingerprint.eq_ignore_ascii_case(fingerprint.trim()))
}

/// Announces this device (`me`) on the LocalSend multicast group and
/// collects the answers for `wait`; returns [`nearby`] afterwards.
///
/// LocalSend apps answer by multicast or, when the compatibility mode runs, by
/// `register` to its server; both end up in the list. The announcement shows
/// this device's name to every LocalSend app on the network, so it is sent
/// only when the user asks for it.
///
/// # Errors
///
/// Fails when the multicast socket cannot be opened.
pub async fn scan(
    engine: &crate::node::NodeEngine,
    me: &OwnDevice,
    wait: Duration,
) -> Result<Vec<LocalSendDevice>> {
    let (own, identity) = own_info(me)?;
    // LocalSend apps answer an announcement with `register` on the
    // announced port; without the compatibility mode nobody would listen
    // there, so a server that only takes `register` runs while scanning. When
    // the port is taken (the mode runs) its server takes them instead.
    let stop = tokio_util::sync::CancellationToken::new();
    let _stop_server = stop.clone().drop_guard();
    match tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, PORT)).await {
        Ok(listener) => {
            let server = Arc::new(RegisterOnly { own: own.clone() });
            tokio::spawn(crate::httpd::serve(
                listener,
                Some(identity.acceptor()?),
                server,
                stop.clone(),
            ));
        }
        Err(e) => debug!("LocalSend port taken while scanning ({e}); the running server answers"),
    }
    let socket = crate::localsend::multicast::bind(PORT).context("LocalSend multicast")?;
    let mut joined = std::collections::BTreeSet::new();
    crate::localsend::multicast::join(engine, &socket, &mut joined);
    let _ = socket.set_multicast_loop_v4(true);
    let _ = socket.set_multicast_ttl_v4(1);
    let interfaces: std::collections::BTreeSet<_> = joined
        .iter()
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
        .copied()
        .collect();
    let message = serde_json::to_vec(&DeviceInfo {
        announce: Some(true),
        ..own.clone()
    })?;
    let deadline = tokio::time::Instant::now() + wait;
    let mut resend = tokio::time::interval(Duration::from_millis(700));
    let mut buf = vec![0u8; 16 * 1024];
    let mut sent = 0;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => break,
            _ = resend.tick(), if sent < 3 => {
                crate::localsend::multicast::send_to_group(&socket, &message, &interfaces).await;
                sent += 1;
            }
            received = socket.recv_from(&mut buf) => {
                if let Ok((len, from)) = received
                    && let Ok(info) = serde_json::from_slice::<DeviceInfo>(&buf[..len])
                {
                    remember_info(&info, from.ip(), &own.fingerprint);
                }
            }
        }
    }
    Ok(nearby())
}

/// The server of [`scan`]: answers `register` and `info`, nothing else.
struct RegisterOnly {
    /// This device's description.
    own: DeviceInfo,
}

impl crate::httpd::Handler for RegisterOnly {
    type Body = Full<Bytes>;

    async fn handle(
        self: Arc<Self>,
        ip: IpAddr,
        req: Request<hyper::body::Incoming>,
    ) -> hyper::Response<Self::Body> {
        let path = req.uri().path().trim_end_matches('/').to_string();
        match (req.method().as_str(), path.as_str()) {
            ("POST", "/api/localsend/v2/register") => {
                match crate::httpd::read_json::<DeviceInfo>(req, 64 * 1024).await {
                    Ok(info) => {
                        remember_info(&info, ip, &self.own.fingerprint);
                        crate::httpd::json(StatusCode::OK, &self.own)
                    }
                    Err(status) => crate::httpd::message(status, "Invalid body"),
                }
            }
            ("GET", "/api/localsend/v2/info") => crate::httpd::json(StatusCode::OK, &self.own),
            _ => crate::httpd::message(StatusCode::NOT_FOUND, "Not found"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(alias: &str, fingerprint: &str) -> DeviceInfo {
        DeviceInfo {
            alias: alias.into(),
            version: "2.1".into(),
            device_model: Some("iPhone".into()),
            fingerprint: fingerprint.into(),
            port: Some(53318),
            protocol: Some("https".into()),
            ..DeviceInfo::default()
        }
    }

    #[test]
    fn devices_are_described_from_what_they_announce() {
        let ip: IpAddr = "192.168.1.20".parse().unwrap();
        let device = LocalSendDevice::from_info(&info("Kate\n's iPhone", "AB12"), ip).unwrap();
        assert_eq!(device.alias, "Kate 's iPhone");
        assert_eq!(device.label(), "Kate 's iPhone (iPhone)");
        assert_eq!(device.address, "192.168.1.20:53318".parse().unwrap());
        assert!(device.https);
        assert_eq!(device.peer_id(), "localsend:192.168.1.20");
        assert!(
            LocalSendDevice::from_info(&info("x", ""), ip).is_none(),
            "no fingerprint"
        );
        let http = DeviceInfo {
            protocol: Some("http".into()),
            port: None,
            ..info("Mac", "CD")
        };
        let device = LocalSendDevice::from_info(&http, ip).unwrap();
        assert!(!device.https);
        assert_eq!(device.address.port(), PORT);
    }

    #[test]
    fn the_own_device_is_not_listed_and_devices_are_found() {
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        remember_info(&info("Self", "OWNFINGERPRINT"), ip, "ownfingerprint");
        assert!(find("Self").is_none());
        remember_info(&info("Living room TV", "FFEE01"), ip, "OWN");
        assert_eq!(
            find("living room tv").map(|d| d.fingerprint),
            Some("FFEE01".into())
        );
        assert!(find("10.1.2.3").is_some());
        assert!(find("ffee01").is_some());
    }
}
