//! State and actions of the terminal user interface.

use anyhow::Result;
#[cfg(not(target_os = "android"))]
use arboard::Clipboard;
use iroh::EndpointId;
use rust_i18n::t;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

use crate::config::{Config, PeerInfo};
use crate::discovery::{Discovery, DiscoveryOptions, NearbyDevice};
use crate::i18n;
use crate::node::NodeEngine;
use crate::pairing::{PairOutcome, PairRequest, clean_device_name, pairing_code, request_pairing};
use crate::protocol::messages::{BatchOffer, MAX_TEXT_LEN, TextDecision, TransferOffer};
use crate::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

/// Time to answer an offer or pairing request before it is rejected.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the configuration is reloaded (changes made by the CLI or GUI).
const CONFIG_RELOAD: Duration = Duration::from_secs(2);

/// What a [`Pending`] decision is about.
pub enum PendingKind {
    /// An incoming file offer.
    File {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// Offered file name.
        file_name: String,
        /// Offered file size in bytes.
        file_size: u64,
    },
    /// Several incoming files announced together.
    Batch {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// Number of files.
        count: u32,
        /// Size of all files in bytes.
        total_size: u64,
        /// Names of the first files.
        names: Vec<String>,
    },
    /// An incoming text for the clipboard.
    Text {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// One-line preview of the text.
        preview: String,
    },
    /// An incoming pairing request.
    Pair {
        /// Name of the requesting device.
        name: String,
        /// Verification code to compare.
        code: String,
    },
}

/// A decision waiting for `y`/`n`.
pub struct Pending {
    /// Subject of the decision.
    pub kind: PendingKind,
    /// Delivers the answer to the waiting handler.
    responder: oneshot::Sender<bool>,
}

impl Pending {
    /// Short description used in status messages.
    fn label(&self) -> &str {
        match &self.kind {
            PendingKind::File { file_name, .. } => file_name,
            PendingKind::Batch { peer_label, .. } => peer_label,
            PendingKind::Text { preview, .. } => preview,
            PendingKind::Pair { name, .. } => name,
        }
    }
}

/// Messages from background tasks to the UI loop.
pub enum UiMessage {
    /// A new decision to show.
    Pending(Pending),
    /// Progress of a transfer.
    Transfer(TransferEvent),
    /// The list of nearby devices changed.
    Nearby(Vec<NearbyDevice>),
    /// Text for the status line.
    Status(String),
    /// Trusted devices changed (e.g. after pairing); carries a status text.
    PeersChanged(String),
    /// An accepted text to copy to the clipboard.
    TextReceived {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// The received text.
        text: String,
    },
}

/// [`TransferHandler`] forwarding offers, pairing requests and progress to the UI loop.
pub struct TuiHandler {
    /// Channel to the UI loop.
    pub tx: mpsc::UnboundedSender<UiMessage>,
}

impl TuiHandler {
    /// Queues a decision in the UI and waits for `y`/`n`; no answer within
    /// [`DECISION_TIMEOUT`] means no.
    async fn ask(&self, kind: PendingKind) -> bool {
        let (responder, answer) = oneshot::channel();
        let _ = self
            .tx
            .send(UiMessage::Pending(Pending { kind, responder }));
        matches!(
            tokio::time::timeout(DECISION_TIMEOUT, answer).await,
            Ok(Ok(true))
        )
    }
}

impl TransferHandler for TuiHandler {
    /// Reloads the configuration per offer, so a changed download folder applies immediately.
    async fn decide(&self, peer: EndpointId, _: &str, offer: &TransferOffer) -> OfferDecision {
        let config = Config::load().unwrap_or_default();
        let dest_dir = config.download_dir();
        let peer_info = config.find_peer(&peer.to_string());
        if peer_info.is_some_and(|p| p.auto_accept) {
            return OfferDecision::Accept { dest_dir };
        }

        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("tui.unknown_device", id = peer.fmt_short()).into_owned());
        let kind = PendingKind::File {
            peer_label,
            file_name: offer.file_name.clone(),
            file_size: offer.file_size,
        };
        if self.ask(kind).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    async fn confirm_pair(&self, req: PairRequest) -> bool {
        let kind = PendingKind::Pair {
            name: req.name.clone(),
            code: req.code.clone(),
        };
        if !self.ask(kind).await {
            return false;
        }
        let saved = Config::load().and_then(|mut c| {
            c.add_paired(req.peer, &req.name, req.addr, false)?;
            c.save()
        });
        let ok = saved.is_ok();
        let _ = self.tx.send(UiMessage::PeersChanged(if ok {
            t!("tui.paired", name = &req.name).into_owned()
        } else {
            t!("tui.pair_save_failed").into_owned()
        }));
        ok
    }

    async fn decide_batch(&self, peer: EndpointId, batch: &BatchOffer) -> Option<OfferDecision> {
        let config = Config::load().unwrap_or_default();
        let dest_dir = config.download_dir();
        let peer_info = config.find_peer(&peer.to_string());
        if peer_info.is_some_and(|p| p.auto_accept) {
            return Some(OfferDecision::Accept { dest_dir });
        }
        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("tui.unknown_device", id = peer.fmt_short()).into_owned());
        let kind = PendingKind::Batch {
            peer_label,
            count: batch.count,
            total_size: batch.total_size,
            names: batch.names.clone(),
        };
        Some(if self.ask(kind).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        })
    }

    async fn receive_text(&self, peer: EndpointId, text: String) -> TextDecision {
        let config = Config::load().unwrap_or_default();
        let peer_info = config.find_peer(&peer.to_string());
        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("tui.unknown_device", id = peer.fmt_short()).into_owned());
        if !peer_info.is_some_and(|p| p.auto_accept) {
            let kind = PendingKind::Text {
                peer_label: peer_label.clone(),
                preview: service::text_preview(&text, 300),
            };
            if !self.ask(kind).await {
                return TextDecision::Rejected {
                    reason: t!("transfer.user_rejected").into_owned(),
                };
            }
        }
        let _ = self.tx.send(UiMessage::TextReceived { peer_label, text });
        TextDecision::Accepted
    }

    fn on_event(&self, event: TransferEvent) {
        let _ = self.tx.send(UiMessage::Transfer(event));
    }
}

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
            DiscoveryOptions::default(),
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
                self.nearby = devices;
                self.clamp_selection();
            }
            UiMessage::Status(status) => self.status_msg = status,
            UiMessage::PeersChanged(status) => {
                self.config = Config::load().unwrap_or_default();
                self.status_msg = status;
                self.clamp_selection();
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

    /// `Enter` / `s`: sends a file to the selected device or opens the selected transfer.
    pub fn activate(&mut self) {
        match self.focus {
            Focus::Trusted => match self.selected_trusted() {
                Some((id, p)) => self.ask_send(id, p.alias),
                None => self.status_msg = t!("tui.no_trusted").into_owned(),
            },
            Focus::Nearby => match self.nearby.get(self.sel_nearby).cloned() {
                Some(d) => self.ask_send(d.node_id, d.name),
                None => self.status_msg = t!("tui.no_nearby").into_owned(),
            },
            Focus::Transfers => self.open_selected_transfer(false),
        }
    }

    /// `T`: sends text or a link to the selected device.
    pub fn ask_send_text(&mut self) {
        let target = match self.focus {
            Focus::Trusted => self.selected_trusted().map(|(id, p)| (id, p.alias)),
            Focus::Nearby => self
                .nearby
                .get(self.sel_nearby)
                .map(|d| (d.node_id.clone(), d.name.clone())),
            Focus::Transfers => None,
        };
        let Some((node_id, label)) = target else {
            self.status_msg = t!("tui.text_where").into_owned();
            return;
        };
        self.open_input(
            InputKind::SendText {
                node_id,
                label: label.clone(),
            },
            format!(" {} ", t!("tui.text_to", name = &label)),
            t!("tui.text_hint").into_owned(),
            String::new(),
        );
    }

    /// Opens the dialog asking for paths of files to send to a device.
    fn ask_send(&mut self, node_id: String, label: String) {
        self.open_input(
            InputKind::SendFile {
                node_id,
                label: label.clone(),
            },
            format!(" {} ", t!("tui.send_to", name = &label)),
            t!("tui.send_hint").into_owned(),
            String::new(),
        );
    }

    /// Opens a text input dialog.
    fn open_input(&mut self, kind: InputKind, title: String, hint: String, value: String) {
        self.input = Some(Input {
            kind,
            title,
            hint,
            value,
            error: None,
        });
    }

    /// Opens the "add device" dialog.
    pub fn open_add_device(&mut self) {
        self.open_input(
            InputKind::AddDevice,
            format!(" {} ", t!("tui.add_title")),
            t!("tui.add_hint").into_owned(),
            String::new(),
        );
    }

    /// Opens the rename dialog for the selected trusted device.
    pub fn open_rename(&mut self) {
        if self.focus != Focus::Trusted {
            return;
        }
        if let Some((id, p)) = self.selected_trusted() {
            self.open_input(
                InputKind::Rename { node_id: id },
                format!(" {} ", t!("tui.rename_title")),
                t!("tui.rename_hint").into_owned(),
                p.alias,
            );
        }
    }

    /// Opens the download folder dialog.
    pub fn open_download_dir(&mut self) {
        let current = self.config.download_dir().display().to_string();
        self.open_input(
            InputKind::DownloadDir,
            format!(" {} ", t!("tui.dir_title")),
            t!("tui.dir_hint").into_owned(),
            current,
        );
    }

    /// Opens the device name dialog.
    pub fn open_device_name(&mut self) {
        let current = self.config.device_name();
        self.open_input(
            InputKind::DeviceName,
            format!(" {} ", t!("tui.name_title")),
            t!("tui.name_hint").into_owned(),
            current,
        );
    }

    /// Switches to the next interface language and saves it.
    pub fn cycle_language(&mut self) {
        let language = i18n::current().next();
        i18n::set_language(language);
        let result = Config::load().and_then(|mut c| {
            c.language = Some(language.code().to_string());
            c.save()?;
            Ok(t!("tui.language", language = language.native_name()).into_owned())
        });
        self.after_config_change(result);
    }

    /// Switches the saved [`NetworkMode`](crate::node::NetworkMode) between
    /// local and internet; it applies after restarting the TUI.
    pub fn toggle_network_mode(&mut self) {
        let result = Config::load().and_then(|mut c| {
            c.network_mode = match c.network_mode {
                crate::node::NetworkMode::Local => crate::node::NetworkMode::Internet,
                crate::node::NetworkMode::Internet => crate::node::NetworkMode::Local,
            };
            c.save()?;
            Ok(t!("tui.network_saved", mode = c.network_mode).into_owned())
        });
        self.after_config_change(result);
    }

    /// Toggles Auto-Accept of the selected trusted device.
    pub fn toggle_auto_accept(&mut self) {
        if self.focus != Focus::Trusted {
            return;
        }
        let Some((id, _)) = self.selected_trusted() else {
            return;
        };
        let result = Config::load().and_then(|mut c| {
            let peer = c
                .peers
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!(t!("tui.device_removed")))?;
            peer.auto_accept = !peer.auto_accept;
            let msg = if peer.auto_accept {
                t!("tui.auto_on", alias = &peer.alias)
            } else {
                t!("tui.auto_off", alias = &peer.alias)
            }
            .into_owned();
            c.save()?;
            Ok(msg)
        });
        self.after_config_change(result);
    }

    /// Asks for confirmation before removing the selected trusted device.
    pub fn ask_remove(&mut self) {
        if self.focus != Focus::Trusted {
            return;
        }
        if let Some((node_id, p)) = self.selected_trusted() {
            self.popup = Some(Popup::ConfirmRemove {
                node_id,
                alias: p.alias,
            });
        }
    }

    /// Removes a trusted device after confirmation.
    pub fn confirm_remove(&mut self, node_id: &str) {
        let result = Config::load().and_then(|mut c| {
            let alias = c.peers.remove(node_id).map(|p| p.alias).unwrap_or_default();
            c.save()?;
            Ok(t!("tui.removed", alias = alias).into_owned())
        });
        self.after_config_change(result);
    }

    /// Reloads the configuration after a change and shows the result in the status line.
    fn after_config_change(&mut self, result: Result<String>) {
        match result {
            Ok(msg) => {
                self.config = Config::load().unwrap_or_default();
                self.status_msg = msg;
                self.clamp_selection();
            }
            Err(e) => self.status_msg = t!("tui.error", error = format!("{e:#}")).into_owned(),
        }
    }

    /// Announces this device immediately to find others faster.
    pub fn refresh_nearby(&mut self) {
        self.status_msg = if let Some(d) = &self.discovery {
            d.probe();
            t!("tui.searching")
        } else {
            t!("tui.discovery_unavailable")
        }
        .into_owned();
    }

    /// Removes finished, failed and rejected transfers from the list.
    pub fn clear_finished(&mut self) {
        self.transfers.retain(|t| {
            matches!(
                t.ev.status,
                TransferStatus::Preparing
                    | TransferStatus::AwaitingDecision
                    | TransferStatus::Running
            )
        });
        self.clamp_selection();
        self.status_msg = t!("tui.cleared").into_owned();
    }

    /// Opens the selected received file, or its folder when `reveal` is set.
    pub fn open_selected_transfer(&mut self, reveal: bool) {
        let Some(row) = self.selected_transfer() else {
            self.status_msg = t!("tui.no_transfers").into_owned();
            return;
        };
        let path = match (&row.ev.status, row.ev.direction) {
            (TransferStatus::Completed { path: Some(p) }, Direction::Download) => PathBuf::from(p),
            (_, Direction::Upload) => {
                self.status_msg = t!("tui.open_upload").into_owned();
                return;
            }
            _ => {
                self.status_msg = t!("tui.open_incomplete").into_owned();
                return;
            }
        };
        let result = if reveal {
            crate::system::reveal_path(&path)
        } else {
            crate::system::open_path(&path)
        };
        self.status_msg = match result {
            Ok(()) => t!("tui.opened", path = path.display()),
            Err(e) => t!("tui.open_failed", error = e),
        }
        .into_owned();
    }

    /// Pairs with the selected nearby device in the background.
    pub fn pair_selected(&mut self) {
        let Some(device) = self.nearby.get(self.sel_nearby).cloned() else {
            self.status_msg = t!("tui.no_nearby").into_owned();
            return;
        };
        self.start_pairing(device.id, device.addr, device.name, None);
    }

    /// Pairs in the background; `keep_alias` keeps an alias chosen when adding the device.
    fn start_pairing(
        &mut self,
        id: EndpointId,
        addr: iroh::EndpointAddr,
        label: String,
        keep_alias: Option<String>,
    ) {
        let code = pairing_code(&self.engine.node_id(), &id);
        self.status_msg = t!("tui.pairing", name = &label, code = code).into_owned();
        let engine = self.engine.clone();
        let my_name = self.config.device_name();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let status = match request_pairing(&engine, addr.clone(), &my_name).await {
                Ok(PairOutcome::Accepted { name, addr: remote }) => {
                    let alias = keep_alias.unwrap_or(name);
                    let saved = Config::load().and_then(|mut c| {
                        c.add_paired(id, &alias, remote.or(Some(addr)), false)?;
                        c.save()
                    });
                    match saved {
                        Ok(()) => t!("tui.paired_mutual", name = &alias),
                        Err(e) => t!("tui.error", error = format!("{e:#}")),
                    }
                }
                Ok(PairOutcome::Rejected) => t!("tui.pair_rejected", name = &label),
                Err(e) => t!("tui.pair_failed", name = &label, error = format!("{e:#}")),
            }
            .into_owned();
            let _ = tx.send(UiMessage::PeersChanged(status));
        });
    }

    /// Copies this device's TICKET to the clipboard.
    pub fn copy_ticket_to_clipboard(&mut self) {
        let ticket = self.ticket_str.clone();
        self.status_msg = if self.set_clipboard(&ticket) {
            t!("tui.copied")
        } else {
            t!("tui.clipboard_unavailable")
        }
        .into_owned();
    }

    /// Puts `text` into the system clipboard; `false` when it is unavailable.
    fn set_clipboard(&mut self, text: &str) -> bool {
        #[cfg(not(target_os = "android"))]
        {
            if self.clipboard.is_none() {
                self.clipboard = Clipboard::new().ok();
            }
            self.clipboard
                .as_mut()
                .is_some_and(|c| c.set_text(text).is_ok())
        }

        #[cfg(target_os = "android")]
        {
            let _ = text;
            false
        }
    }

    /// Answers the first pending decision.
    pub fn answer_pending(&mut self, accept: bool) {
        if let Some(p) = self.pending.pop_front() {
            let label = p.label().to_string();
            let _ = p.responder.send(accept);
            self.status_msg = if accept {
                t!("tui.accepted", name = label)
            } else {
                t!("tui.declined", name = label)
            }
            .into_owned();
        }
    }

    /// `Tab` in a path field: completes the path.
    pub fn input_complete(&mut self) {
        if let Some(input) = self.input.as_mut()
            && input.kind.is_path()
            && let Some(completed) = complete_path(&input.value)
        {
            input.value = completed;
            input.error = None;
        }
    }

    /// `Enter` in the input dialog; on a validation error the dialog stays open.
    pub fn submit_input(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        let value = input.value.trim().to_string();
        let result: Result<(), String> = match &input.kind {
            InputKind::SendFile { node_id, label } => self.submit_send(node_id, label, &value),
            InputKind::SendText { node_id, label } => self.submit_text(node_id, label, &value),
            InputKind::Rename { node_id } => self.submit_rename(node_id, &value),
            InputKind::AddDevice => self.submit_add_device(&value),
            InputKind::DownloadDir => self.submit_download_dir(&value),
            InputKind::DeviceName => self.submit_device_name(&value),
        };
        if let Err(error) = result {
            self.input = Some(Input {
                error: Some(error),
                ..input
            });
        }
    }

    /// Address of a device; see [`crate::discovery::resolve_target`].
    fn target_addr(&self, node_id: &str) -> Result<iroh::EndpointAddr, String> {
        crate::discovery::resolve_target(&self.config, self.discovery.as_ref(), node_id)
            .map_err(|e| t!("tui.error", error = format!("{e:#}")).into_owned())
    }

    /// Sends text to a device in the background; the result goes to the status line.
    fn submit_text(&mut self, node_id: &str, label: &str, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Err(t!("service.text_empty").into_owned());
        }
        if value.len() > MAX_TEXT_LEN {
            return Err(t!("service.text_too_long", max = MAX_TEXT_LEN).into_owned());
        }
        let addr = self.target_addr(node_id)?;
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        let text = value.to_string();
        let label = label.to_string();
        self.status_msg = t!("tui.text_sending", name = &label).into_owned();
        tokio::spawn(async move {
            let status = match service::send_text(&engine, addr, &text).await {
                Ok(TextDecision::Accepted) => t!("tui.text_delivered", name = &label),
                Ok(TextDecision::Rejected { reason }) => {
                    t!("tui.text_rejected", name = &label, reason = reason)
                }
                Err(e) => t!("tui.send_failed", name = &label, error = format!("{e:#}")),
            }
            .into_owned();
            let _ = tx.send(UiMessage::Status(status));
        });
        Ok(())
    }

    /// Validates the paths and sends the files in the background.
    fn submit_send(&mut self, node_id: &str, label: &str, value: &str) -> Result<(), String> {
        let paths = parse_paths(value);
        if paths.is_empty() {
            return Err(t!("tui.path_required").into_owned());
        }
        if let Some(bad) = paths.iter().find(|p| !p.is_file()) {
            return Err(t!("tui.not_a_file", path = bad.display()).into_owned());
        }
        let addr = self.target_addr(node_id)?;
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        let label = label.to_string();
        self.status_msg = t!("tui.sending", count = paths.len(), name = &label).into_owned();
        tokio::spawn(async move {
            let sink_tx = tx.clone();
            let sink = move |ev| {
                let _ = sink_tx.send(UiMessage::Transfer(ev));
            };
            let status = match service::send_files(&engine, addr, &paths, &sink).await {
                Ok(()) => t!("tui.sent", name = &label),
                Err(e) => t!("tui.send_failed", name = &label, error = format!("{e:#}")),
            }
            .into_owned();
            let _ = tx.send(UiMessage::Status(status));
        });
        self.focus = Focus::Transfers;
        Ok(())
    }

    /// Saves a new alias of a trusted device.
    fn submit_rename(&mut self, node_id: &str, value: &str) -> Result<(), String> {
        let alias = clean_device_name(value);
        if alias.is_empty() {
            return Err(t!("settings.name_empty").into_owned());
        }
        let result = Config::load().and_then(|mut c| {
            let peer = c
                .peers
                .get_mut(node_id)
                .ok_or_else(|| anyhow::anyhow!(t!("tui.device_removed")))?;
            peer.alias = alias.clone();
            c.save()?;
            Ok(t!("tui.renamed", alias = &alias).into_owned())
        });
        self.after_config_change(result);
        Ok(())
    }

    /// Adds the device and sends it a pairing request, so that after confirmation
    /// the other side trusts this device too.
    fn submit_add_device(&mut self, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Err(t!("tui.add_required").into_owned());
        }
        let mut config = Config::load().map_err(|e| e.to_string())?;
        let node_id = config
            .add_from_input(value, "", false)
            .map_err(|e| format!("{e:#}"))?;
        if node_id == self.node_id {
            return Err(t!("tui.own_ticket").into_owned());
        }
        config.save().map_err(|e| e.to_string())?;
        let alias = config
            .find_peer(&node_id)
            .map(|p| p.alias.clone())
            .unwrap_or_default();
        self.config = config;
        let addr = self.target_addr(&node_id)?;
        self.focus = Focus::Trusted;
        self.start_pairing(addr.id, addr, alias.clone(), Some(alias));
        Ok(())
    }

    /// Creates and saves a new download folder.
    fn submit_download_dir(&mut self, value: &str) -> Result<(), String> {
        let dir = expand_path(value);
        if dir.as_os_str().is_empty() {
            return Err(t!("tui.dir_required").into_owned());
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| t!("tui.dir_create_failed", error = e).into_owned())?;
        let result = Config::load().and_then(|mut c| {
            c.default_download_dir = Some(dir.clone());
            c.save()?;
            Ok(t!("tui.dir_changed", path = dir.display()).into_owned())
        });
        self.after_config_change(result);
        Ok(())
    }

    /// Saves and announces a new name of this device.
    fn submit_device_name(&mut self, value: &str) -> Result<(), String> {
        let name = clean_device_name(value);
        if name.is_empty() {
            return Err(t!("settings.name_empty").into_owned());
        }
        let result = Config::load().and_then(|mut c| {
            c.device_name = Some(name.clone());
            c.save()?;
            Ok(t!("settings.name", name = &name).into_owned())
        });
        if let Some(d) = &self.discovery {
            d.set_name(&name);
        }
        self.after_config_change(result);
        Ok(())
    }
}

/// Expands `~`, strips quotes and a `file://` prefix and decodes `%XX` escapes.
fn expand_path(raw: &str) -> PathBuf {
    let raw = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    let raw = raw.strip_prefix("file://").unwrap_or(raw);
    let decoded = percent_decode(raw);
    if let Some(rest) = decoded.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    if decoded == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(decoded)
}

/// Minimal `%XX` decoding for `file://` paths pasted from file managers.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses one path or several (e.g. files dragged into a terminal, quoted or
/// escaped with `\ `).
pub fn parse_paths(raw: &str) -> Vec<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let single = expand_path(raw);
    if single.exists() {
        return vec![single];
    }
    shell_split(raw).iter().map(|p| expand_path(p)).collect()
}

/// Splits like a shell: whitespace separates, quotes group, `\` escapes a character.
fn shell_split(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', None) => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ('"' | '\'', None) => quote = Some(c),
            (c, Some(q)) if c == q => quote = None,
            (c, None) if c.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            (c, _) => current.push(c),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Path completion: a single match is completed fully, several matches to
/// their common prefix.
fn complete_path(value: &str) -> Option<String> {
    let (dir_part, prefix) = match value.rfind('/') {
        Some(i) => (&value[..=i], &value[i + 1..]),
        None => ("", value),
    };
    let lookup_dir = if dir_part.is_empty() {
        PathBuf::from(".")
    } else {
        expand_path(dir_part)
    };
    let mut matches: Vec<(String, bool)> = std::fs::read_dir(&lookup_dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let hidden_ok = prefix.starts_with('.') || !name.starts_with('.');
            (hidden_ok && name.starts_with(prefix)).then(|| {
                let is_dir = e.path().is_dir();
                (name, is_dir)
            })
        })
        .collect();
    matches.sort();
    match matches.as_slice() {
        [] => None,
        [(name, is_dir)] => Some(format!(
            "{dir_part}{name}{}",
            if *is_dir { "/" } else { "" }
        )),
        many => {
            let first = &many[0].0;
            let common = many.iter().fold(first.len(), |len, (name, _)| {
                first
                    .chars()
                    .zip(name.chars())
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a.len_utf8())
                    .sum::<usize>()
                    .min(len)
            });
            (common > prefix.len()).then(|| format!("{dir_part}{}", &first[..common]))
        }
    }
}

/// Shortens paths inside the home folder to `~/…` for display.
pub fn display_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_dropped_paths() {
        assert_eq!(
            shell_split(r#"'/tmp/a b.txt' /tmp/c\ d.txt "/tmp/e.txt""#),
            vec!["/tmp/a b.txt", "/tmp/c d.txt", "/tmp/e.txt"]
        );
    }

    #[test]
    fn expands_file_urls_and_quotes() {
        assert_eq!(
            expand_path("'file:///tmp/my%20ph%C3%B6to.jpg'"),
            PathBuf::from("/tmp/my phöto.jpg")
        );
    }

    #[test]
    fn completes_paths() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("report-2026.pdf"), b"x")?;
        std::fs::write(dir.path().join("report-2025.pdf"), b"x")?;
        std::fs::create_dir(dir.path().join("photos"))?;
        let base = format!("{}/", dir.path().display());
        assert_eq!(
            complete_path(&format!("{base}rep")),
            Some(format!("{base}report-202"))
        );
        assert_eq!(
            complete_path(&format!("{base}ph")),
            Some(format!("{base}photos/"))
        );
        assert_eq!(complete_path(&format!("{base}missing")), None);
        Ok(())
    }
}
