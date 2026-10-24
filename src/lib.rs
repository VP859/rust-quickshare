//! # RQS P2P Quick Share
//!
//! Secure peer-to-peer file sharing over QUIC for Linux, Windows, macOS and
//! Android. Files go directly between devices: no storage server, no cloud,
//! no mDNS or Bluetooth. Connections use [iroh](https://docs.rs/iroh) (TLS 1.3
//! with Ed25519 identities, UDP hole punching and an encrypted relay fallback),
//! so transfers keep working behind NAT and with a VPN enabled.
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
//! - [`cli`], [`tui`], [`daemon`] – the terminal user interfaces of the `rqs` binary.
//!
//! ## Example
//!
//! Sending a file to a device whose TICKET is known:
//!
//! ```no_run
//! use rqs_p2p::node::{NodeEngine, NodeTicket};
//! use rqs_p2p::transfer::service;
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

#![warn(missing_docs)]

rust_i18n::i18n!("locales", fallback = "en");

pub mod cli;
pub mod config;
pub mod daemon;
pub mod discovery;
pub mod i18n;
pub mod node;
pub mod pairing;
pub mod protocol;
pub mod system;
pub mod telemetry;
pub mod transfer;
pub mod tui;
