use anyhow::Result;
#[cfg(not(target_os = "android"))]
use arboard::Clipboard;
use std::collections::HashMap;

use crate::config::Config;
use crate::node::{NodeEngine, NodeTicket, load_or_generate_secret_key};

#[derive(Debug, Clone)]
pub struct TransferItem {
    pub id: String,
    pub file_name: String,
    pub file_size: u64,
    pub bytes_transferred: u64,
    pub speed_mbps: f64,
    pub is_upload: bool,
    pub status: String,
}

pub struct App {
    pub node_id: String,
    pub ticket_str: String,
    pub status_msg: String,
    pub config: Config,
    pub transfers: HashMap<String, TransferItem>,
    pub pending_offer: Option<(String, String, u64)>,
}

impl App {
    pub async fn new() -> Result<Self> {
        let config = Config::load().unwrap_or_default();
        let secret_key = load_or_generate_secret_key(None)?;
        let engine = NodeEngine::new(secret_key).await?;

        let node_id = engine.node_id().to_string();
        let addr = engine.node_addr().await?;
        let ticket_str = NodeTicket::new(addr)
            .serialize_to_string()
            .unwrap_or_default();

        Ok(Self {
            node_id,
            ticket_str,
            status_msg: "Węzeł gotowy. Naciśnij [c] aby skopiować TICKET, [q] aby wyjść."
                .to_string(),
            config,
            transfers: HashMap::new(),
            pending_offer: None,
        })
    }

    pub fn copy_ticket_to_clipboard(&mut self) {
        #[cfg(not(target_os = "android"))]
        {
            if let Ok(mut clipboard) = Clipboard::new()
                && clipboard.set_text(&self.ticket_str).is_ok()
            {
                self.status_msg = "Skopiowano TICKET do schowka!".to_string();
                return;
            }
            self.status_msg = "Błąd kopiowania do schowka.".to_string();
        }

        // Na Androidzie TUI nie działa – schowek obsługuje frontend WebView
        // (navigator.clipboard / tauri-plugin-clipboard-manager).
        #[cfg(target_os = "android")]
        {
            self.status_msg = "Schowek niedostępny w trybie tekstowym na Androidzie."
                .to_string();
        }
    }

    pub fn accept_pending_offer(&mut self) {
        if self.pending_offer.take().is_some() {
            self.status_msg = "Zaakceptowano transfer.".to_string();
        }
    }

    pub fn reject_pending_offer(&mut self) {
        if self.pending_offer.take().is_some() {
            self.status_msg = "Odrzucono transfer.".to_string();
        }
    }

    pub fn update(&mut self) {}
}
