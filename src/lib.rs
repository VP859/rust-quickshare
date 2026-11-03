//! # Ferry
//!
//! Secure peer-to-peer file sharing over QUIC for Linux, Windows and Android.
//! Files go directly between devices: no storage server, no cloud, no mDNS or
//! Bluetooth. Connections use [iroh](https://docs.rs/iroh) (TLS 1.3 with
//! Ed25519 identities) and reach devices at the addresses announced on the
//! local network or carried by a TICKET. By default no external servers are
//! contacted; the optional [`node::NetworkMode::Internet`] adds the public iroh
//! relays for devices in different networks.
//!
//! ## Overview
//!
//! - [`node`] – the P2P endpoint, the persistent device identity and TICKETs.
//! - [`protocol`] – wire messages and their framing.
//! - [`transfer`] – sending and receiving files with per-chunk BLAKE3
//!   verification, resume of interrupted downloads and file name sanitization;
//!   [`transfer::service`] is the node loop shared by every user interface.
//! - [`discovery`] – finding devices on the local network.
//! - [`pairing`] – one-button pairing with a verification code.
//! - [`config`] – trusted devices and settings in `config.toml`.
//! - [`i18n`] – interface language (English, Polish).
//! - [`error`] – classifying errors ([`error::kind`]).
//! - [`size`], [`system`] – readable sizes, opening files and links.
//!
//! The `ferry` binary (CLI, terminal UI, daemon) lives in the `ferry-cli`
//! crate and the desktop/Android app in `ferry-gui`; both only use this API.
//!
//! ## Example
//!
//! Sending a file to a device whose TICKET is known:
//!
//! ```no_run
//! use ferry::node::{NodeEngine, NodeTicket};
//! use ferry::transfer::service;
//!
//! # async fn example(ticket: &str) -> anyhow::Result<()> {
//! let engine = NodeEngine::with_persistent_identity().await?;
//! let target = NodeTicket::parse_target(ticket)?;
//! service::send_files(&engine, target, &["report.pdf".into()], &|event| {
//!     println!("{} {:?}", event.file_name, event.status);
//! })
//! .await?;
//! # Ok(())
//! # }
//! ```

rust_i18n::i18n!("locales", fallback = "en");

pub mod config;
pub mod discovery;
pub mod error;
pub mod i18n;
pub mod localsend;
pub mod node;
pub mod pairing;
pub mod protocol;
pub mod size;
pub mod system;
pub mod transfer;
