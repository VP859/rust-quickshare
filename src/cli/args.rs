//! Command line arguments of the `ferry` binary.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Ferry – secure P2P file sharing over QUIC, without a cloud or mDNS.
#[derive(Parser, Debug)]
#[command(name = "ferry", author, version, long_about = None)]
pub struct Cli {
    /// Interface language for this run (`en`, `pl`); overrides the saved setting.
    #[arg(long, global = true, value_name = "CODE")]
    pub lang: Option<String>,

    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level subcommands.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Send file(s) to a trusted or nearby device (`--to`), or share one file by a one-time TICKET.
    Send {
        /// Files to send.
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Alias, nearby device name, Node ID or TICKET of the receiver
        /// (it must run `ferry daemon`, the TUI or the GUI).
        #[arg(short, long)]
        to: Option<String>,
    },
    /// Send text or a link; the receiver copies it to the clipboard after accepting.
    Text {
        /// Text to send (default: read from standard input).
        text: Option<String>,
        /// Alias, nearby device name, Node ID or TICKET of the receiver.
        #[arg(short, long)]
        to: String,
    },
    /// Download a file shared by TICKET.
    Receive {
        /// Share TICKET printed by `ferry send` or shown by the GUI.
        ticket: String,
        /// Destination folder (default: the download folder).
        output_dir: Option<PathBuf>,
    },
    /// Run in the background; trusted devices with Auto-Accept are accepted automatically.
    Daemon {
        /// Folder for received files (default: the saved setting or the download folder).
        #[arg(short, long)]
        downloads_dir: Option<PathBuf>,
    },
    /// Interactive terminal user interface.
    Tui,
    /// Show this device's TICKET (text and QR code) for pairing.
    Ticket,
    /// Search for Ferry devices on the local network (own UDP broadcast, no mDNS).
    Discover {
        /// Search duration in seconds.
        #[arg(short, long, default_value_t = 5)]
        timeout: u64,
    },
    /// Pair with a nearby device (name or Node ID from `ferry discover`); the other
    /// side confirms and both show the same code.
    Pair {
        /// Name or Node ID of a nearby device, or a TICKET / Node ID.
        target: String,
        /// Enable Auto-Accept for the paired device right away.
        #[arg(long, default_value_t = false)]
        auto_accept: bool,
    },
    /// Show or change the name of this device visible to others.
    Name {
        /// New device name.
        name: Option<String>,
    },
    /// Show or change the interface language (`en`, `pl`).
    Language {
        /// Language code to save.
        code: Option<String>,
    },
    /// Show or change the network mode: `local` (no external servers, default)
    /// or `internet` (also through the free public iroh relays, for devices in
    /// different networks).
    Network {
        /// Mode to save.
        mode: Option<String>,
    },
    /// Manage trusted devices.
    Peer {
        /// Peer subcommand.
        #[command(subcommand)]
        command: PeerCommands,
    },
    /// Network and configuration diagnostics.
    Doctor,
}

/// Subcommands of `ferry peer`.
#[derive(Subcommand, Debug)]
pub enum PeerCommands {
    /// Add a device by TICKET or Node ID and send it a pairing request.
    Add {
        /// TICKET or Node ID of the device.
        ticket: String,
        /// Name for the device.
        #[arg(short, long)]
        alias: String,
        /// Accept files from this device without asking.
        #[arg(long, default_value_t = false)]
        auto_accept: bool,
        /// Do not send a pairing request (trust only on this side).
        #[arg(long, default_value_t = false)]
        no_pair: bool,
    },
    /// List trusted devices.
    List,
    /// Change the alias or Auto-Accept setting of a device.
    Edit {
        /// Alias or Node ID.
        target: String,
        /// New alias.
        #[arg(short, long)]
        alias: Option<String>,
        /// New Auto-Accept setting.
        #[arg(long)]
        auto_accept: Option<bool>,
    },
    /// Remove a trusted device.
    Remove {
        /// Alias or Node ID.
        target: String,
    },
}
