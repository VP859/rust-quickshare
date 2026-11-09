//! Command line arguments of the `ferry` binary.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Ferry – secure P2P file sharing over QUIC, without a cloud or mDNS.
#[derive(Parser, Debug)]
#[command(
    name = "ferry",
    author,
    version,
    long_about = None,
    after_help = "New to Ferry? `ferry guide` explains the first steps in a few lines."
)]
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
    /// Send files and folders to a trusted or nearby device (`--to`), or share one file by a
    /// one-time TICKET.
    Send {
        /// Files and folders to send (folders keep their structure).
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Alias, nearby device name, Node ID or TICKET of the receiver
        /// (it must run `ferry daemon`, the TUI or the GUI). Repeat it to send
        /// to several devices at once.
        #[arg(short, long)]
        to: Vec<String>,
    },
    /// Send files to LocalSend apps (iPhone, iPad, Mac, …) and find them on the local network.
    Localsend {
        /// LocalSend subcommand.
        #[command(subcommand)]
        command: LocalsendCommands,
    },
    /// Add "Send with Ferry" to the file managers of this user (Dolphin, Nautilus, Nemo; Linux).
    Integrate {
        /// Remove the entries again.
        #[arg(long)]
        remove: bool,
    },
    /// Show finished transfers, newest first, or search them.
    History {
        /// Show only transfers whose file name, device or detail contains this text.
        query: Option<String>,
        /// How many transfers to show.
        #[arg(short = 'n', long, default_value_t = 30)]
        limit: usize,
        /// Show only texts and links (never their content), or only files.
        #[arg(long, value_enum)]
        kind: Option<HistoryFilter>,
        /// Delete the history.
        #[arg(long)]
        clear: bool,
        /// Send the file of transfer N of the list (as numbered) again to the
        /// same Ferry device.
        #[arg(long, value_name = "N", conflicts_with = "clear")]
        send_again: Option<usize>,
    },
    /// Share files with a web browser on the local network and receive files from it, through a
    /// one-time link and QR code (the other device needs no app).
    Web {
        /// Files the browser can download (none: only receive).
        files: Vec<PathBuf>,
        /// Do not let the browser send files here.
        #[arg(long)]
        no_receive: bool,
        /// Minutes until the link stops working.
        #[arg(long, default_value_t = 30)]
        minutes: u64,
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
    /// Let unknown devices send pairing requests for a few minutes, although
    /// `paired-only` is on (also for a running daemon or TUI); `--close` ends it.
    Pairing {
        /// How long pairing stays open, in minutes (at most 30).
        #[arg(short, long, default_value_t = 5)]
        minutes: u32,
        /// Close pairing now.
        #[arg(long, default_value_t = false)]
        close: bool,
    },
    /// Short answers to the usual questions: sending, pairing, devices without Ferry,
    /// iPhone and Mac, local or internet, devices that do not show up, privacy.
    Guide {
        /// Only this topic: send, pair, browser, apple, internet, trouble, privacy.
        topic: Option<String>,
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
        /// Mode to save: `local`, `internet` or `relay`.
        mode: Option<String>,
        /// With `relay`: the relay URLs (`https://relay.example.org`); without
        /// them the saved ones are kept.
        relays: Vec<String>,
    },
    /// Manage trusted devices.
    Peer {
        /// Peer subcommand.
        #[command(subcommand)]
        command: PeerCommands,
    },
    /// Show or change settings (`ferry config list`, `get`, `set`, `unset`, `path`).
    Config {
        /// Config subcommand (default: `list`).
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
    /// Show, back up or restore this device's identity (its Node ID key).
    Identity {
        /// Identity subcommand (default: `show`).
        #[command(subcommand)]
        command: Option<IdentityCommands>,
    },
    /// Network and configuration diagnostics.
    Doctor,
}

/// What `ferry history` shows.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryFilter {
    /// Files and folders only.
    Files,
    /// Texts and links only.
    Texts,
}

/// Settings that `ferry config` can read and change.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKey {
    /// Name of this device visible to others.
    DeviceName,
    /// Interface language code.
    Language,
    /// `local` or `internet`.
    NetworkMode,
    /// Folder for received files.
    DownloadDir,
    /// Hidden on the local network from unpaired devices (`true`/`false`).
    Hidden,
    /// Accept files, texts and pairing only from paired devices (`true`/`false`).
    PairedOnly,
    /// Send photos (JPEG, PNG) without their location and other metadata (`true`/`false`).
    StripMetadata,
    /// Tell everyone on the local network which operating system this is (`true`/`false`).
    AnnouncePlatform,
    /// Listen on a random port instead of the fixed 47824; applies at the next start (`true`/`false`).
    RandomPort,
    /// Copy texts from Auto-Accept devices without asking (`true`/`false`).
    AutoAcceptTexts,
    /// Accept programs, scripts and macro documents from Auto-Accept devices without asking (`true`/`false`).
    AutoAcceptExecutables,
    /// Desktop app: quit when the window is closed instead of staying in the tray (`true`/`false`).
    QuitOnClose,
    /// Relays of the `relay` network mode, separated by commas (`https://relay.example.org`).
    RelayUrls,
    /// Keep a history of transfers (`true`/`false`).
    History,
    /// Speed limit for sending and, separately, receiving (`500K`, `2M`, `off`).
    SpeedLimit,
    /// Receive files from LocalSend apps – iPhone, Mac – without pairing (`true`/`false`).
    Localsend,
}

/// Subcommands of `ferry config`.
#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Show every setting.
    List,
    /// Show one setting.
    Get {
        /// Setting name.
        key: SettingKey,
    },
    /// Change one setting.
    Set {
        /// Setting name.
        key: SettingKey,
        /// New value.
        value: String,
    },
    /// Reset one setting to its default.
    Unset {
        /// Setting name.
        key: SettingKey,
    },
    /// Print the path of the configuration file.
    Path,
}

/// Subcommands of `ferry identity`.
#[derive(Subcommand, Debug)]
pub enum IdentityCommands {
    /// Show the Node ID and where the key is stored.
    Show,
    /// Write a backup of the key to a new file (keep it secret).
    Export {
        /// File to create; an existing file is never overwritten.
        file: PathBuf,
    },
    /// Show or change where the key is kept: `file` (default) or `system`
    /// (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows).
    Storage {
        /// Where to move the key; without it the current place is shown.
        storage: Option<String>,
    },
    /// Replace this device's key with one from a backup.
    Import {
        /// Backup created by `ferry identity export`.
        file: PathBuf,
        /// Replace an existing, different key (it is kept as a `.bak` file).
        #[arg(long)]
        force: bool,
    },
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
    /// Show the safety number shared with a trusted device, to compare it with
    /// the other device; `--confirm` records that both match.
    Verify {
        /// Alias or Node ID.
        target: String,
        /// The numbers are equal: mark the device as verified.
        #[arg(long, conflicts_with = "unconfirm")]
        confirm: bool,
        /// Remove the verified mark.
        #[arg(long)]
        unconfirm: bool,
    },
    /// Remove a trusted device.
    Remove {
        /// Alias or Node ID.
        target: String,
    },
}

/// Subcommands of `ferry localsend`.
#[derive(Subcommand, Debug)]
pub enum LocalsendCommands {
    /// List the LocalSend apps on the local network. Announces this device's
    /// name to them once, so they answer.
    Scan {
        /// Seconds to wait for answers.
        #[arg(short, long, default_value_t = 3)]
        timeout: u64,
    },
    /// Send files and folders to a LocalSend app. Its name is not verified:
    /// anyone on the network can announce any name.
    Send {
        /// Files and folders to send (folders keep their structure).
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Name or IP address (`192.168.1.20` or `192.168.1.20:53317`) of the
        /// LocalSend app, as listed by `ferry localsend scan`.
        #[arg(short, long)]
        to: String,
        /// PIN, for apps that require one.
        #[arg(long)]
        pin: Option<String>,
    },
}
