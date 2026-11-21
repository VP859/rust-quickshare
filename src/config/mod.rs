//! Persistent configuration stored in `config.toml`.

pub mod check;
pub mod paths;
pub mod peers;
pub mod signature;

pub use peers::{Config, PeerInfo};
