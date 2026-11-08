//! State of the terminal user interface.
//!
//! [`App`] holds what is shown; its methods are spread over the submodules by
//! topic: [`handler`] (questions from the node), [`actions`] (keys), [`input`]
//! (text fields) and [`paths`] (typed and dropped file paths).

use anyhow::Result;
#[cfg(not(target_os = "android"))]
use arboard::Clipboard;
use iroh::EndpointId;
use rust_i18n::t;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

use ferry::config::{Config, PeerInfo};
use ferry::discovery::{Discovery, DiscoveryOptions, NearbyDevice, Visibility};
use ferry::i18n;
use ferry::node::NodeEngine;
use ferry::pairing::{PairOutcome, PairRequest, clean_device_name, request_pairing};
use ferry::protocol::messages::{BatchOffer, MAX_TEXT_LEN, TextDecision, TransferOffer};
use ferry::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

/// Time to answer an offer or pairing request before it is rejected.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the configuration is reloaded (changes made by the CLI or GUI).
const CONFIG_RELOAD: Duration = Duration::from_secs(2);

mod actions;
mod handler;
mod input;
mod paths;

pub use handler::{Pending, PendingKind, TuiHandler, UiMessage};
pub use paths::display_path;
use paths::{complete_path, expand_path, parse_paths};

/// The focused panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Trusted devices.
    Trusted,
    /// Devices on the local network.
    Nearby,
    /// Transfer list.
    Transfers,
}

impl Focus {
    /// The following (or, with `back`, the previous) panel.
    fn next(self, back: bool) -> Self {
        match (self, back) {
            (Focus::Trusted, false) | (Focus::Transfers, true) => Focus::Nearby,
            (Focus::Nearby, false) | (Focus::Trusted, true) => Focus::Transfers,
            (Focus::Transfers, false) | (Focus::Nearby, true) => Focus::Trusted,
        }
    }
}

/// Purpose of the text input dialog.
pub enum InputKind {
    /// Paths of files to send to a device.
    SendFile {
        /// Receiver's Node ID.
        node_id: String,
        /// Receiver's display name.
        label: String,
    },
    /// Text or a link to send to a device.
    SendText {
        /// Receiver's Node ID.
        node_id: String,
        /// Receiver's display name.
        label: String,
    },
    /// New alias of a trusted device.
    Rename {
        /// Node ID of the device.
        node_id: String,
    },
    /// TICKET or Node ID of a device to add.
    AddDevice,
    /// New download folder.
    DownloadDir,
    /// New name of this device.
    DeviceName,
}

impl InputKind {
    /// Whether the field holds paths, enabling `Tab` completion.
    pub fn is_path(&self) -> bool {
        matches!(self, InputKind::SendFile { .. } | InputKind::DownloadDir)
    }
}

/// An open text input dialog.
pub struct Input {
    /// Purpose of the dialog.
    pub kind: InputKind,
    /// Dialog title.
    pub title: String,
    /// Explanation shown above the field.
    pub hint: String,
    /// Current text.
    pub value: String,
    /// Validation error of the last submit attempt.
    pub error: Option<String>,
}

/// An open informational dialog.
pub enum Popup {
    /// This device's TICKET with a QR code.
    Ticket,
    /// Keyboard shortcuts.
    Help,
    /// The safety number shared with a trusted device, to compare with the
    /// other device's screen.
    SafetyNumber {
        /// Node ID of the device.
        node_id: String,
        /// Alias shown in the title.
        alias: String,
        /// 12 groups of 5 digits.
        number: String,
        /// The user already verified it.
        verified: bool,
    },
    /// Confirmation of removing a trusted device.
    ConfirmRemove {
        /// Node ID of the device.
        node_id: String,
        /// Alias shown in the question.
        alias: String,
    },
}

/// A transfer shown in the list, with its smoothed speed.
pub struct TransferRow {
    /// Latest state.
    pub ev: TransferEvent,
    /// Smoothed speed in bytes per second.
    pub speed: f64,
    /// Bytes done and time of the last speed sample.
    last_sample: (u64, Instant),
}

/// The whole TUI state.
pub struct App {
    /// The P2P node.
    pub engine: NodeEngine,
    /// Local network discovery, if it could start.
    pub discovery: Option<Discovery>,
    /// This device's Node ID.
    pub node_id: String,
    /// This device's pairing TICKET.
    pub ticket_str: String,
    /// Status line text.
    pub status_msg: String,
    /// Current configuration.
    pub config: Config,
    /// Transfers in order of appearance (newest last).
    pub transfers: Vec<TransferRow>,
    /// Decisions waiting for the user (first shown on top).
    pub pending: VecDeque<Pending>,
    /// Devices on the local network.
    pub nearby: Vec<NearbyDevice>,
    /// Focused panel.
    pub focus: Focus,
    /// Selection in the trusted devices panel.
    pub sel_trusted: usize,
    /// Selection in the nearby devices panel.
    pub sel_nearby: usize,
    /// Selection in the transfers panel (0 = newest).
    pub sel_transfer: usize,
    /// Open text input dialog.
    pub input: Option<Input>,
    /// Open informational dialog.
    pub popup: Option<Popup>,
    /// Explanation why no nearby devices are visible.
    pub discovery_hint: Option<String>,
    /// Channel for messages from background tasks.
    tx: mpsc::UnboundedSender<UiMessage>,
    /// Time of the last configuration reload.
    last_reload: Instant,
    /// On Linux the clipboard content disappears with this object, so it lives
    /// as long as the TUI.
    #[cfg(not(target_os = "android"))]
    clipboard: Option<Clipboard>,
}

impl App {
    /// Starts the node and discovery and builds the initial state.
    ///
    /// # Errors
    ///
    /// Fails when the node cannot start.
    pub async fn new(tx: mpsc::UnboundedSender<UiMessage>) -> Result<Self> {
        let config = Config::load().unwrap_or_default();
        let engine = NodeEngine::with_persistent_identity().await?;
        let node_id = engine.node_id().to_string();
        let ticket_str = engine.ticket().await?.serialize_to_string()?;
        let discovery = Discovery::start(
            engine.clone(),
            &config.device_name(),
            DiscoveryOptions::from_config(&config),
        )
        .ok();

        Ok(Self {
            engine,
            discovery,
            node_id,
            ticket_str,
            status_msg: t!("tui.ready").into_owned(),
            config,
            transfers: Vec::new(),
            pending: VecDeque::new(),
            nearby: Vec::new(),
            focus: Focus::Trusted,
            sel_trusted: 0,
            sel_nearby: 0,
            sel_transfer: 0,
            input: None,
            popup: None,
            discovery_hint: None,
            tx,
            last_reload: Instant::now(),
            #[cfg(not(target_os = "android"))]
            clipboard: None,
        })
    }

    /// Trusted devices sorted by alias (a stable order for navigation).
    pub fn trusted(&self) -> Vec<(String, PeerInfo)> {
        let mut list: Vec<_> = self
            .config
            .peers
            .iter()
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect();
        list.sort_by_key(|(_, p)| p.alias.to_lowercase());
        list
    }

    /// Whether the device is currently visible on the local network.
    pub fn is_online(&self, node_id: &str) -> bool {
        self.nearby.iter().any(|d| d.node_id == node_id)
    }

    /// Called every frame: periodically reloads the configuration and the discovery hint.
    pub fn tick(&mut self) {
        if self.last_reload.elapsed() >= CONFIG_RELOAD {
            self.last_reload = Instant::now();
            if let Ok(config) = Config::load() {
                self.config = config;
            }
            self.discovery_hint = match &self.discovery {
                Some(d) => d.health().hint(),
                None => Some(t!("tui.discovery_unavailable").into_owned()),
            };
            self.clamp_selection();
        }
    }

    /// Keeps every selection inside its list after the lists changed.
    fn clamp_selection(&mut self) {
        let max = |len: usize| len.saturating_sub(1);
        self.sel_trusted = self.sel_trusted.min(max(self.config.peers.len()));
        self.sel_nearby = self.sel_nearby.min(max(self.nearby.len()));
        self.sel_transfer = self.sel_transfer.min(max(self.transfers.len()));
    }

    /// Applies a message from a background task.
    pub fn handle_message(&mut self, msg: UiMessage) {
        match msg {
            UiMessage::Pending(p) => {
                self.status_msg = match &p.kind {
                    PendingKind::File { file_name, .. } => {
                        t!("tui.new_offer", name = file_name).into_owned()
                    }
                    PendingKind::Batch { count, .. } => {
                        t!("tui.new_batch", count = count).into_owned()
                    }
                    PendingKind::Text { peer_label, .. } => {
                        t!("tui.new_text", name = peer_label).into_owned()
                    }
                    PendingKind::Pair { name, .. } => {
                        t!("tui.new_pair_request", name = name).into_owned()
                    }
                };
                self.pending.push_back(p);
            }
            UiMessage::Transfer(ev) => self.update_transfer(ev),
            UiMessage::Nearby(devices) => {
                let engine = self.engine.clone();
                let seen = devices.clone();
                tokio::spawn(async move {
                    crate::cli::peer_cmd::deliver_pending_unpairs(&engine, &seen).await;
                });
                self.nearby = devices;
                self.clamp_selection();
            }
            UiMessage::Status(status) => self.status_msg = status,
            UiMessage::PeersChanged(status) => {
                self.reload_config();
                self.status_msg = status;
            }
            UiMessage::TextReceived { peer_label, text } => {
                let preview = service::text_preview(&text, 80);
                self.status_msg = if self.set_clipboard(&text) {
                    t!("tui.text_copied", name = peer_label, text = preview)
                } else {
                    t!("tui.text_not_copied", name = peer_label, text = preview)
                }
                .into_owned();
            }
        }
    }

    /// Applies a transfer event and updates the smoothed speed of its row.
    fn update_transfer(&mut self, ev: TransferEvent) {
        let now = Instant::now();
        match self.transfers.iter_mut().find(|t| t.ev.id == ev.id) {
            Some(row) => {
                let (last_done, last_at) = row.last_sample;
                let dt = now.duration_since(last_at).as_secs_f64();
                if ev.status == TransferStatus::Running && dt >= 0.5 && ev.done >= last_done {
                    let current = (ev.done - last_done) as f64 / dt;
                    row.speed = if row.speed == 0.0 {
                        current
                    } else {
                        0.6 * row.speed + 0.4 * current
                    };
                    row.last_sample = (ev.done, now);
                }
                if ev.status != TransferStatus::Running {
                    row.speed = 0.0;
                }
                row.ev = ev;
            }
            None => {
                self.transfers.push(TransferRow {
                    last_sample: (ev.done, now),
                    speed: 0.0,
                    ev,
                });
                self.sel_transfer = 0;
            }
        }
    }

    /// Moves the focus to the next (or previous) panel.
    pub fn cycle_focus(&mut self, back: bool) {
        self.focus = self.focus.next(back);
    }

    /// Moves the selection in the focused panel, wrapping around.
    pub fn select(&mut self, delta: isize) {
        let (sel, len) = match self.focus {
            Focus::Trusted => (&mut self.sel_trusted, self.config.peers.len()),
            Focus::Nearby => (&mut self.sel_nearby, self.nearby.len()),
            Focus::Transfers => (&mut self.sel_transfer, self.transfers.len()),
        };
        if len > 0 {
            *sel = (*sel as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    /// Node ID and settings of the selected trusted device.
    fn selected_trusted(&self) -> Option<(String, PeerInfo)> {
        self.trusted().get(self.sel_trusted).cloned()
    }

    /// The selected transfer; the list is shown newest first.
    pub fn selected_transfer(&self) -> Option<&TransferRow> {
        let len = self.transfers.len();
        (self.sel_transfer < len).then(|| &self.transfers[len - 1 - self.sel_transfer])
    }
}
