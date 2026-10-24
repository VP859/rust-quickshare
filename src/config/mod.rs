//! Persistent configuration stored in `config.toml`.

pub mod paths;
pub mod peers;

pub use peers::{Config, PeerInfo};
