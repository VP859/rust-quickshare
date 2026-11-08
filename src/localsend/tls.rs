//! The TLS certificate of the LocalSend compatibility mode.
//!
//! LocalSend devices identify each other by the SHA-256 fingerprint of their
//! self-signed certificate: a sender pins the fingerprint announced over
//! multicast, so the certificate must stay the same across restarts. It is
//! created once and kept next to the device key (`localsend.cert.der`,
//! `localsend.key.der`, the key readable only by the owner). It is separate
//! from the Ferry identity: LocalSend devices never learn the Node ID.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// File name of the certificate (DER) in the configuration folder.
const CERT_FILE: &str = "localsend.cert.der";
/// File name of the private key (PKCS#8 DER) in the configuration folder.
const KEY_FILE: &str = "localsend.key.der";

/// Certificate and key of the LocalSend server.
pub struct Identity {
    /// Self-signed certificate, DER.
    cert: Vec<u8>,
    /// Private key, PKCS#8 DER.
    key: Vec<u8>,
}

impl Identity {
    /// Loads the identity from `dir`, creating it on first use.
    ///
    /// # Errors
    ///
    /// Fails when the files cannot be read or written, or a new certificate
    /// cannot be created.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            let identity = Self { cert, key };
            if identity.acceptor().is_ok() {
                return Ok(identity);
            }
            tracing::warn!("LocalSend certificate is unusable, creating a new one");
        }
        let identity = Self::generate()?;
        std::fs::create_dir_all(dir)?;
        for path in [&cert_path, &key_path] {
            let _ = std::fs::remove_file(path);
        }
        crate::node::identity::write_private(&key_path, &identity.key)
            .with_context(|| format!("cannot write {}", key_path.display()))?;
        std::fs::write(&cert_path, &identity.cert)
            .with_context(|| format!("cannot write {}", cert_path.display()))?;
        Ok(identity)
    }

    /// A new self-signed certificate (ECDSA P-256), not saved anywhere.
    ///
    /// # Errors
    ///
    /// Fails only if key generation fails.
    pub fn generate() -> Result<Self> {
        let key = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::default();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Ferry");
        let cert = params.self_signed(&key)?;
        Ok(Self {
            cert: cert.der().to_vec(),
            key: key.serialize_der(),
        })
    }

    /// The fingerprint LocalSend devices know this device by: SHA-256 of the
    /// DER certificate as uppercase hex.
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.cert)
    }

    /// TLS acceptor serving this certificate; client certificates are not requested.
    ///
    /// # Errors
    ///
    /// Fails when the key does not match the certificate.
    pub fn acceptor(&self) -> Result<TlsAcceptor> {
        let config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.cert.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone())),
            )?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

impl Identity {
    /// Certificate chain and key to present as a TLS client: LocalSend apps
    /// ask the sender for its certificate (whose fingerprint the offer names).
    pub fn client_auth(&self) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        (
            vec![CertificateDer::from(self.cert.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone())),
        )
    }
}

/// SHA-256 of `der` as uppercase hex, the LocalSend fingerprint format.
pub fn fingerprint_of(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_is_kept_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let again = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_eq!(first.fingerprint().len(), 64);
        assert!(
            first
                .fingerprint()
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_damaged_certificate_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        std::fs::write(dir.path().join(KEY_FILE), b"garbage").unwrap();
        let second = Identity::load_or_create(dir.path()).unwrap();
        assert_ne!(first.fingerprint(), second.fingerprint());
        assert!(second.acceptor().is_ok());
    }

    #[test]
    fn fingerprint_matches_the_official_format() {
        // SHA-256 of the empty input, uppercase hex.
        assert_eq!(
            fingerprint_of(b""),
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
    }
}
