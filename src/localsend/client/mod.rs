//! Sending files to LocalSend apps (protocol v2, the sending side).
//!
//! LocalSend devices are found in three ways, all collected in one list
//! ([`nearby`]): their multicast announcements, their `register` requests to
//! the compatibility mode's server, and [`scan`], which announces this device
//! once and listens for the answers. A device can also be given by its IP
//! address ([`LocalSendDevice::probe`]), for networks without multicast.
//!
//! [`send`] then offers the files (`prepare-upload`), waits for the user on
//! the other device, and uploads every accepted file. Over HTTPS the
//! certificate must have the fingerprint the device announced, as in the
//! official app.
//!
//! **Security.** As for receiving, this is LocalSend's security model: the
//! receiver is not paired and its name is whatever it announces. Anyone on the
//! local network can announce a device with the name of someone else's phone;
//! the fingerprint only binds the connection to the announcement, not to a
//! person. Interfaces say so before sending.
//!
//! The parts: the apps nearby in `devices`, sending in `outgoing`, and the HTTPS
//! client in `http`.

mod devices;
mod http;
mod outgoing;

pub(crate) use devices::remember_info;
pub use devices::{LocalSendDevice, changes, find, find_by_fingerprint, nearby, scan};
pub use outgoing::{PinRequired, SendResult, send, send_text};

use crate::localsend::dto::DeviceInfo;
use crate::localsend::{PORT, PROTOCOL_VERSION};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// How this device presents itself to LocalSend apps.
#[derive(Debug, Clone, Default)]
pub struct OwnDevice {
    /// Device name shown to the other user.
    pub name: String,
    /// Folder of the compatibility mode's certificate; `None` is the
    /// configuration folder (tests: a temporary folder).
    pub cert_dir: Option<PathBuf>,
}

impl OwnDevice {
    /// This device named `name`, with the certificate in the configuration folder.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            cert_dir: None,
        }
    }
}

/// This device as described to LocalSend apps, and its certificate: the one
/// of the compatibility mode (created on first use), so an app that knows this
/// device from receiving recognizes it when it sends too.
fn own_info(own: &OwnDevice) -> Result<(DeviceInfo, Arc<super::tls::Identity>)> {
    let dir = match &own.cert_dir {
        Some(dir) => dir.clone(),
        None => crate::config::paths::config_dir()?,
    };
    let identity = Arc::new(super::tls::Identity::load_or_create(&dir)?);
    let info = DeviceInfo {
        alias: crate::pairing::clean_device_name(&own.name),
        version: PROTOCOL_VERSION.to_string(),
        device_model: Some("Ferry".to_string()),
        device_type: Some(
            if cfg!(target_os = "android") {
                "mobile"
            } else {
                "desktop"
            }
            .into(),
        ),
        fingerprint: identity.fingerprint(),
        port: Some(PORT),
        protocol: Some("https".to_string()),
        download: false,
        announce: None,
    };
    Ok((info, identity))
}
