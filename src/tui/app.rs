use anyhow::Result;
#[cfg(not(target_os = "android"))]
use arboard::Clipboard;
use iroh::EndpointId;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

use crate::config::{Config, PeerInfo};
use crate::discovery::{Discovery, DiscoveryOptions, NearbyDevice};
use crate::node::NodeEngine;
use crate::pairing::{PairOutcome, PairRequest, clean_device_name, pairing_code, request_pairing};
use crate::protocol::messages::TransferOffer;
use crate::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};

/// Czas na decyzję w TUI, po którym oferta jest odrzucana.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
/// Odświeżanie konfiguracji (zmiany z CLI/GUI na tym komputerze).
const CONFIG_RELOAD: Duration = Duration::from_secs(2);

pub enum PendingKind {
    File {
        peer_label: String,
        file_name: String,
        file_size: u64,
    },
    Pair {
        name: String,
        code: String,
    },
}

/// Decyzja oczekująca na [y]/[n] – oferta pliku lub prośba o sparowanie.
pub struct Pending {
    pub kind: PendingKind,
    responder: oneshot::Sender<bool>,
}

impl Pending {
    fn label(&self) -> &str {
        match &self.kind {
            PendingKind::File { file_name, .. } => file_name,
            PendingKind::Pair { name, .. } => name,
        }
    }
}

pub enum UiMessage {
    Pending(Pending),
    Transfer(TransferEvent),
    Nearby(Vec<NearbyDevice>),
    /// Komunikat na pasku stanu.
    Status(String),
    /// Zmieniła się lista zaufanych urządzeń (np. po parowaniu).
    PeersChanged(String),
}

/// Handler węzła przekazujący oferty, parowanie i postęp do pętli TUI.
pub struct TuiHandler {
    pub tx: mpsc::UnboundedSender<UiMessage>,
}

impl TuiHandler {
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
    async fn decide(&self, peer: EndpointId, _: &str, offer: &TransferOffer) -> OfferDecision {
        // Konfiguracja czytana przy każdej ofercie – zmiana katalogu zapisu działa od razu.
        let config = Config::load().unwrap_or_default();
        let dest_dir = config.download_dir();
        let peer_info = config.find_peer(&peer.to_string());
        if peer_info.is_some_and(|p| p.auto_accept) {
            return OfferDecision::Accept { dest_dir };
        }

        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| format!("nieznane urządzenie {}", peer.fmt_short()));
        let kind = PendingKind::File {
            peer_label,
            file_name: offer.file_name.clone(),
            file_size: offer.file_size,
        };
        if self.ask(kind).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: "Użytkownik odrzucił transfer".into(),
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
            format!("Sparowano z '{}'", req.name)
        } else {
            "Błąd zapisu sparowanego urządzenia".to_string()
        }));
        ok
    }

    fn on_event(&self, event: TransferEvent) {
        let _ = self.tx.send(UiMessage::Transfer(event));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Trusted,
    Nearby,
    Transfers,
}

impl Focus {
    fn next(self, back: bool) -> Self {
        match (self, back) {
            (Focus::Trusted, false) | (Focus::Transfers, true) => Focus::Nearby,
            (Focus::Nearby, false) | (Focus::Trusted, true) => Focus::Transfers,
            (Focus::Transfers, false) | (Focus::Nearby, true) => Focus::Trusted,
        }
    }
}

pub enum InputKind {
    SendFile { node_id: String, label: String },
    Rename { node_id: String },
    AddDevice,
    DownloadDir,
    DeviceName,
}

impl InputKind {
    /// Pola ze ścieżkami mają podpowiadanie klawiszem Tab.
    pub fn is_path(&self) -> bool {
        matches!(self, InputKind::SendFile { .. } | InputKind::DownloadDir)
    }
}

pub struct Input {
    pub kind: InputKind,
    pub title: String,
    pub hint: String,
    pub value: String,
    pub error: Option<String>,
}

pub enum Popup {
    Ticket,
    Help,
    ConfirmRemove { node_id: String, alias: String },
}

pub struct TransferRow {
    pub ev: TransferEvent,
    /// Prędkość w B/s (wygładzana).
    pub speed: f64,
    last_sample: (u64, Instant),
}

pub struct App {
    pub engine: NodeEngine,
    pub discovery: Option<Discovery>,
    pub node_id: String,
    pub ticket_str: String,
    pub status_msg: String,
    pub config: Config,
    /// Transfery w kolejności pojawienia się (najnowsze na końcu).
    pub transfers: Vec<TransferRow>,
    pub pending: VecDeque<Pending>,
    pub nearby: Vec<NearbyDevice>,
    pub focus: Focus,
    pub sel_trusted: usize,
    pub sel_nearby: usize,
    pub sel_transfer: usize,
    pub input: Option<Input>,
    pub popup: Option<Popup>,
    pub discovery_hint: Option<String>,
    tx: mpsc::UnboundedSender<UiMessage>,
    last_reload: Instant,
    /// Na Linuksie zawartość schowka znika razem z obiektem – trzymamy go przez cały czas pracy.
    #[cfg(not(target_os = "android"))]
    clipboard: Option<Clipboard>,
}

impl App {
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
            status_msg: "Węzeł gotowy. [?] pomoc, [t] TICKET, [i] dodaj urządzenie.".to_string(),
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

    /// Zaufane urządzenia posortowane po nazwie (stała kolejność do nawigacji).
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

    pub fn is_online(&self, node_id: &str) -> bool {
        self.nearby.iter().any(|d| d.node_id == node_id)
    }

    /// Wywoływane w każdej klatce: odświeżenie konfiguracji i diagnostyki wykrywania.
    pub fn tick(&mut self) {
        if self.last_reload.elapsed() >= CONFIG_RELOAD {
            self.last_reload = Instant::now();
            if let Ok(config) = Config::load() {
                self.config = config;
            }
            self.discovery_hint = match &self.discovery {
                Some(d) => d.health().hint(),
                None => Some("Wykrywanie w sieci lokalnej jest niedostępne.".into()),
            };
            self.clamp_selection();
        }
    }

    fn clamp_selection(&mut self) {
        let max = |len: usize| len.saturating_sub(1);
        self.sel_trusted = self.sel_trusted.min(max(self.config.peers.len()));
        self.sel_nearby = self.sel_nearby.min(max(self.nearby.len()));
        self.sel_transfer = self.sel_transfer.min(max(self.transfers.len()));
    }

    pub fn handle_message(&mut self, msg: UiMessage) {
        match msg {
            UiMessage::Pending(p) => {
                self.status_msg = match &p.kind {
                    PendingKind::File { file_name, .. } => format!("Nowa oferta: {file_name}"),
                    PendingKind::Pair { name, .. } => format!("Prośba o sparowanie: {name}"),
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
        }
    }

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
                // Nowy transfer na górze listy (lista jest wyświetlana od najnowszego).
                self.sel_transfer = 0;
            }
        }
    }

    // ---------------- nawigacja ----------------

    pub fn cycle_focus(&mut self, back: bool) {
        self.focus = self.focus.next(back);
    }

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

    fn selected_trusted(&self) -> Option<(String, PeerInfo)> {
        self.trusted().get(self.sel_trusted).cloned()
    }

    /// Transfery wyświetlane są od najnowszego – indeks zaznaczenia liczony od końca.
    pub fn selected_transfer(&self) -> Option<&TransferRow> {
        let len = self.transfers.len();
        (self.sel_transfer < len).then(|| &self.transfers[len - 1 - self.sel_transfer])
    }

    // ---------------- akcje ----------------

    /// Enter / `s` – zależnie od aktywnego panelu.
    pub fn activate(&mut self) {
        match self.focus {
            Focus::Trusted => match self.selected_trusted() {
                Some((id, p)) => self.ask_send(id, p.alias),
                None => self.status_msg = "Brak zaufanych urządzeń – [i] dodaj urządzenie.".into(),
            },
            Focus::Nearby => match self.nearby.get(self.sel_nearby).cloned() {
                Some(d) => self.ask_send(d.node_id, d.name),
                None => self.status_msg = "Brak urządzeń w pobliżu.".into(),
            },
            Focus::Transfers => self.open_selected_transfer(false),
        }
    }

    fn ask_send(&mut self, node_id: String, label: String) {
        self.open_input(
            InputKind::SendFile {
                node_id,
                label: label.clone(),
            },
            format!(" Wyślij do: {label} "),
            "Ścieżka pliku (Tab – podpowiedź, możesz przeciągnąć plik do okna terminala)".into(),
            String::new(),
        );
    }

    fn open_input(&mut self, kind: InputKind, title: String, hint: String, value: String) {
        self.input = Some(Input {
            kind,
            title,
            hint,
            value,
            error: None,
        });
    }

    pub fn open_add_device(&mut self) {
        self.open_input(
            InputKind::AddDevice,
            " Dodaj urządzenie ".into(),
            "Wklej TICKET lub Node ID drugiego urządzenia – wyślemy też prośbę o sparowanie".into(),
            String::new(),
        );
    }

    pub fn open_rename(&mut self) {
        if self.focus != Focus::Trusted {
            return;
        }
        if let Some((id, p)) = self.selected_trusted() {
            self.open_input(
                InputKind::Rename { node_id: id },
                " Zmień nazwę urządzenia ".into(),
                "Nowa nazwa (alias)".into(),
                p.alias,
            );
        }
    }

    pub fn open_download_dir(&mut self) {
        let current = self.config.download_dir().display().to_string();
        self.open_input(
            InputKind::DownloadDir,
            " Katalog zapisu odebranych plików ".into(),
            "Ścieżka katalogu (zostanie utworzony, jeśli nie istnieje)".into(),
            current,
        );
    }

    pub fn open_device_name(&mut self) {
        let current = self.config.device_name();
        self.open_input(
            InputKind::DeviceName,
            " Nazwa tego urządzenia ".into(),
            "Widoczna dla innych w „W pobliżu” i przy parowaniu".into(),
            current,
        );
    }

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
                .ok_or_else(|| anyhow::anyhow!("Urządzenie zostało usunięte"))?;
            peer.auto_accept = !peer.auto_accept;
            let msg = format!(
                "Auto-Accept {} dla '{}'",
                if peer.auto_accept {
                    "włączony"
                } else {
                    "wyłączony"
                },
                peer.alias
            );
            c.save()?;
            Ok(msg)
        });
        self.after_config_change(result);
    }

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

    pub fn confirm_remove(&mut self, node_id: &str) {
        let result = Config::load().and_then(|mut c| {
            let alias = c.peers.remove(node_id).map(|p| p.alias).unwrap_or_default();
            c.save()?;
            Ok(format!("Usunięto urządzenie '{alias}'"))
        });
        self.after_config_change(result);
    }

    fn after_config_change(&mut self, result: Result<String>) {
        match result {
            Ok(msg) => {
                self.config = Config::load().unwrap_or_default();
                self.status_msg = msg;
                self.clamp_selection();
            }
            Err(e) => self.status_msg = format!("Błąd: {e:#}"),
        }
    }

    pub fn refresh_nearby(&mut self) {
        if let Some(d) = &self.discovery {
            d.probe();
            self.status_msg = "Szukanie urządzeń w sieci lokalnej...".into();
        } else {
            self.status_msg = "Wykrywanie w sieci lokalnej jest niedostępne.".into();
        }
    }

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
        self.status_msg = "Usunięto zakończone transfery z listy.".into();
    }

    /// `o`/Enter – otwórz plik, `f` – otwórz folder.
    pub fn open_selected_transfer(&mut self, reveal: bool) {
        let Some(row) = self.selected_transfer() else {
            self.status_msg = "Brak transferów.".into();
            return;
        };
        let path = match (&row.ev.status, row.ev.direction) {
            (TransferStatus::Completed { path: Some(p) }, Direction::Download) => PathBuf::from(p),
            (_, Direction::Upload) => {
                self.status_msg = "To plik wysłany – otwierać można pliki odebrane.".into();
                return;
            }
            _ => {
                self.status_msg = "Plik nie został jeszcze odebrany w całości.".into();
                return;
            }
        };
        let result = if reveal {
            crate::system::reveal_path(&path)
        } else {
            crate::system::open_path(&path)
        };
        self.status_msg = match result {
            Ok(()) => format!("Otwarto: {}", path.display()),
            Err(e) => format!("Nie udało się otworzyć: {e}"),
        };
    }

    /// Paruje z zaznaczonym urządzeniem „W pobliżu” (w tle – TUI pozostaje responsywne).
    pub fn pair_selected(&mut self) {
        let Some(device) = self.nearby.get(self.sel_nearby).cloned() else {
            self.status_msg = "Brak urządzeń w pobliżu do sparowania.".into();
            return;
        };
        self.start_pairing(device.id, device.addr, device.name, None);
    }

    /// Parowanie w tle; `keep_alias` – alias nadany ręcznie przy dodawaniu urządzenia.
    fn start_pairing(
        &mut self,
        id: EndpointId,
        addr: iroh::EndpointAddr,
        label: String,
        keep_alias: Option<String>,
    ) {
        let code = pairing_code(&self.engine.node_id(), &id);
        self.status_msg =
            format!("Parowanie z '{label}' – kod {code}. Potwierdź na drugim urządzeniu...");
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
                        Ok(()) => format!("Sparowano obustronnie z '{alias}'"),
                        Err(e) => format!("Błąd zapisu: {e:#}"),
                    }
                }
                Ok(PairOutcome::Rejected) => format!("'{label}' odrzuciło parowanie"),
                Err(e) => format!("Parowanie z '{label}' nieudane: {e:#}"),
            };
            let _ = tx.send(UiMessage::PeersChanged(status));
        });
    }

    pub fn copy_ticket_to_clipboard(&mut self) {
        #[cfg(not(target_os = "android"))]
        {
            if self.clipboard.is_none() {
                self.clipboard = Clipboard::new().ok();
            }
            let copied = self
                .clipboard
                .as_mut()
                .is_some_and(|c| c.set_text(&self.ticket_str).is_ok());
            self.status_msg = if copied {
                "Skopiowano TICKET do schowka (pozostaje dostępny, dopóki działa TUI).".to_string()
            } else {
                "Schowek niedostępny – [t] pokaże TICKET do zaznaczenia.".to_string()
            };
        }

        #[cfg(target_os = "android")]
        {
            self.status_msg = "Schowek niedostępny w trybie tekstowym na Androidzie.".to_string();
        }
    }

    pub fn answer_pending(&mut self, accept: bool) {
        if let Some(p) = self.pending.pop_front() {
            let label = p.label().to_string();
            let _ = p.responder.send(accept);
            self.status_msg = if accept {
                format!("Zaakceptowano: {label}")
            } else {
                format!("Odrzucono: {label}")
            };
        }
    }

    // ---------------- pole tekstowe ----------------

    pub fn input_complete(&mut self) {
        if let Some(input) = self.input.as_mut()
            && input.kind.is_path()
            && let Some(completed) = complete_path(&input.value)
        {
            input.value = completed;
            input.error = None;
        }
    }

    pub fn submit_input(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        let value = input.value.trim().to_string();
        let result: Result<(), String> = match &input.kind {
            InputKind::SendFile { node_id, label } => self.submit_send(node_id, label, &value),
            InputKind::Rename { node_id } => self.submit_rename(node_id, &value),
            InputKind::AddDevice => self.submit_add_device(&value),
            InputKind::DownloadDir => self.submit_download_dir(&value),
            InputKind::DeviceName => self.submit_device_name(&value),
        };
        if let Err(error) = result {
            // Pole zostaje otwarte z komunikatem błędu.
            self.input = Some(Input {
                error: Some(error),
                ..input
            });
        }
    }

    fn submit_send(&mut self, node_id: &str, label: &str, value: &str) -> Result<(), String> {
        let paths = parse_paths(value);
        if paths.is_empty() {
            return Err("Podaj ścieżkę pliku".into());
        }
        if let Some(bad) = paths.iter().find(|p| !p.is_file()) {
            return Err(format!("To nie jest plik: {}", bad.display()));
        }
        let fallback = self.config.resolve_target(node_id);
        let nearby = self.discovery.as_ref().and_then(|d| {
            EndpointId::from_str(node_id)
                .ok()
                .and_then(|id| d.addr_of(&id))
        });
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        let node_id = node_id.to_string();
        let label = label.to_string();
        let count = paths.len();
        self.status_msg = format!("Wysyłanie {count} plik(ów) do '{label}'...");
        tokio::spawn(async move {
            let addr = match (nearby, fallback) {
                (Some(addr), _) => addr,
                (None, Ok(addr)) => addr,
                (None, Err(_)) => match iroh::EndpointId::from_str(&node_id) {
                    Ok(id) => id.into(),
                    Err(e) => {
                        let _ = tx.send(UiMessage::Status(format!("Błąd adresu: {e}")));
                        return;
                    }
                },
            };
            let sink_tx = tx.clone();
            let sink = move |ev| {
                let _ = sink_tx.send(UiMessage::Transfer(ev));
            };
            let status = match service::send_files(&engine, addr, &paths, &sink).await {
                Ok(()) => format!("Wysłano do '{label}'"),
                Err(e) => format!("Wysyłka do '{label}': {e:#}"),
            };
            let _ = tx.send(UiMessage::Status(status));
        });
        self.focus = Focus::Transfers;
        Ok(())
    }

    fn submit_rename(&mut self, node_id: &str, value: &str) -> Result<(), String> {
        let alias = clean_device_name(value);
        if alias.is_empty() {
            return Err("Nazwa nie może być pusta".into());
        }
        let result = Config::load().and_then(|mut c| {
            let peer = c
                .peers
                .get_mut(node_id)
                .ok_or_else(|| anyhow::anyhow!("Urządzenie zostało usunięte"))?;
            peer.alias = alias.clone();
            c.save()?;
            Ok(format!("Zmieniono nazwę na '{alias}'"))
        });
        self.after_config_change(result);
        Ok(())
    }

    fn submit_add_device(&mut self, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Err("Wklej TICKET lub Node ID".into());
        }
        let mut config = Config::load().map_err(|e| e.to_string())?;
        let node_id = config
            .add_from_input(value, "", false)
            .map_err(|e| format!("{e:#}"))?;
        if node_id == self.node_id {
            return Err("To jest TICKET tego urządzenia".into());
        }
        config.save().map_err(|e| e.to_string())?;
        let alias = config
            .find_peer(&node_id)
            .map(|p| p.alias.clone())
            .unwrap_or_default();
        let addr = config.resolve_target(&node_id).map_err(|e| e.to_string())?;
        let addr = self
            .discovery
            .as_ref()
            .and_then(|d| d.addr_of(&addr.id))
            .unwrap_or(addr);
        self.config = config;
        self.focus = Focus::Trusted;
        // Prośba o sparowanie – po akceptacji druga strona też doda to urządzenie.
        self.start_pairing(addr.id, addr, alias.clone(), Some(alias));
        Ok(())
    }

    fn submit_download_dir(&mut self, value: &str) -> Result<(), String> {
        let dir = expand_path(value);
        if dir.as_os_str().is_empty() {
            return Err("Podaj ścieżkę katalogu".into());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("Nie można utworzyć katalogu: {e}"))?;
        let result = Config::load().and_then(|mut c| {
            c.default_download_dir = Some(dir.clone());
            c.save()?;
            Ok(format!("Pliki będą zapisywane w {}", dir.display()))
        });
        self.after_config_change(result);
        Ok(())
    }

    fn submit_device_name(&mut self, value: &str) -> Result<(), String> {
        let name = clean_device_name(value);
        if name.is_empty() {
            return Err("Nazwa nie może być pusta".into());
        }
        let result = Config::load().and_then(|mut c| {
            c.device_name = Some(name.clone());
            c.save()?;
            Ok(format!("Nazwa urządzenia: {name}"))
        });
        if let Some(d) = &self.discovery {
            d.set_name(&name);
        }
        self.after_config_change(result);
        Ok(())
    }
}

/// `~` → katalog domowy, `file://` i cudzysłowy usunięte.
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

/// Minimalne dekodowanie `%20` itp. (ścieżki `file://` z menedżerów plików).
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

/// Jedna ścieżka lub kilka (np. przeciągnięte do terminala: z cudzysłowami lub `\ `).
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

/// Podział jak w powłoce: spacje rozdzielają, cudzysłowy grupują, `\` escapuje znak.
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

/// Podpowiedź ścieżki (Tab): jedno dopasowanie – pełna nazwa, kilka – wspólny prefiks.
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

/// Czy ścieżka jest pod katalogiem domowym – do skróconego wyświetlania (`~/...`).
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
            expand_path("'file:///tmp/moje%20zdj%C4%99cie.jpg'"),
            PathBuf::from("/tmp/moje zdjęcie.jpg")
        );
    }

    #[test]
    fn completes_paths() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("raport-2026.pdf"), b"x")?;
        std::fs::write(dir.path().join("raport-2025.pdf"), b"x")?;
        std::fs::create_dir(dir.path().join("zdjecia"))?;
        let base = format!("{}/", dir.path().display());
        assert_eq!(
            complete_path(&format!("{base}rap")),
            Some(format!("{base}raport-202"))
        );
        assert_eq!(
            complete_path(&format!("{base}zd")),
            Some(format!("{base}zdjecia/"))
        );
        assert_eq!(complete_path(&format!("{base}brak")), None);
        Ok(())
    }
}
