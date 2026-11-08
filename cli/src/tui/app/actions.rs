//! Actions triggered by keys: sending, devices, settings, transfers, pairing.

use super::*;

impl App {
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

    /// Switches the saved [`NetworkMode`](ferry::node::NetworkMode): local →
    /// internet → own relay (only when relays are set) → local; it applies
    /// after restarting the TUI.
    pub fn toggle_network_mode(&mut self) {
        use ferry::node::NetworkMode;
        let result = Config::load().and_then(|mut c| {
            c.network_mode = match c.network_mode {
                NetworkMode::Local => NetworkMode::Internet,
                NetworkMode::Internet if !c.relay_urls.is_empty() => NetworkMode::Relay,
                NetworkMode::Internet | NetworkMode::Relay => NetworkMode::Local,
            };
            c.save()?;
            Ok(t!("tui.network_saved", mode = c.network_mode).into_owned())
        });
        self.after_config_change(result);
    }

    /// Hides this device from unpaired devices on the local network, or shows
    /// it again; applies immediately.
    pub fn toggle_hidden(&mut self) {
        let result = Config::load().and_then(|mut c| {
            c.hidden = !c.hidden;
            c.save()?;
            Ok(if c.hidden {
                t!("tui.hidden_on")
            } else {
                t!("tui.hidden_off")
            }
            .into_owned())
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

    /// Shows the safety number shared with the selected trusted device.
    pub fn show_safety_number(&mut self) {
        if self.focus != Focus::Trusted {
            self.status_msg = t!("tui.safety_where").into_owned();
            return;
        }
        let Some((node_id, peer)) = self.selected_trusted() else {
            return;
        };
        let Ok(id) = node_id.parse::<EndpointId>() else {
            return;
        };
        self.popup = Some(Popup::SafetyNumber {
            number: ferry::pairing::safety_number(&self.engine.node_id(), &id),
            verified: peer.verified,
            alias: peer.alias,
            node_id,
        });
    }

    /// Records that the safety number of `node_id` was (or no longer is) compared.
    pub fn set_verified(&mut self, node_id: &str, verified: bool) {
        let result = Config::load().and_then(|mut c| {
            c.set_verified(node_id, verified);
            c.save()?;
            Ok(if verified {
                t!("tui.verified_on")
            } else {
                t!("tui.verified_off")
            }
            .into_owned())
        });
        self.after_config_change(result);
    }

    /// Removes a trusted device after confirmation.
    pub fn confirm_remove(&mut self, node_id: &str) {
        let mut target = None;
        let result = Config::load().and_then(|mut c| {
            target = c.resolve_target(node_id).ok();
            let alias = c.peers.remove(node_id).map(|p| p.alias).unwrap_or_default();
            c.queue_unpair(node_id);
            c.save()?;
            Ok(t!("tui.removed", alias = alias).into_owned())
        });
        // Tell the other device (best effort), so that it stops listing this one.
        if result.is_ok()
            && let Some(target) = target
        {
            let engine = self.engine.clone();
            let node_id = node_id.to_string();
            tokio::spawn(async move {
                if ferry::transfer::service::send_unpair(&engine, target)
                    .await
                    .is_ok()
                {
                    crate::cli::peer_cmd::forget_unpair(&node_id);
                }
            });
        }
        self.after_config_change(result);
    }

    /// Reloads the configuration and applies the paired devices and visibility
    /// to discovery.
    pub(super) fn reload_config(&mut self) {
        self.config = Config::load().unwrap_or_default();
        if let Some(discovery) = &self.discovery {
            discovery.set_visibility(Visibility::from_config(&self.config));
        }
        self.clamp_selection();
    }

    /// Reloads the configuration after a change and shows the result in the status line.
    pub(super) fn after_config_change(&mut self, result: Result<String>) {
        match result {
            Ok(msg) => {
                self.reload_config();
                self.status_msg = msg;
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
        self.transfers.retain(|t| !t.ev.status.is_final());
        self.clamp_selection();
        self.status_msg = t!("tui.cleared").into_owned();
    }

    /// Cancels the selected transfer if it is still running.
    pub fn cancel_selected_transfer(&mut self) {
        let Some(row) = self.selected_transfer() else {
            self.status_msg = t!("tui.no_transfers").into_owned();
            return;
        };
        self.status_msg = if !row.ev.status.is_final() && service::cancel_transfer(&row.ev.id) {
            t!("tui.cancelling", name = &row.ev.file_name)
        } else {
            t!("tui.cannot_cancel")
        }
        .into_owned();
    }

    /// Pauses the selected running transfer, or resumes it when paused.
    pub fn pause_selected_transfer(&mut self) {
        let Some(row) = self.selected_transfer() else {
            self.status_msg = t!("tui.no_transfers").into_owned();
            return;
        };
        let pause = match row.ev.status {
            TransferStatus::Paused => false,
            TransferStatus::Running => true,
            _ => {
                self.status_msg = t!("tui.cannot_pause").into_owned();
                return;
            }
        };
        self.status_msg = if service::pause_transfer(&row.ev.id, pause) {
            if pause {
                t!("tui.pausing", name = &row.ev.file_name)
            } else {
                t!("tui.resuming", name = &row.ev.file_name)
            }
        } else {
            t!("tui.cannot_pause")
        }
        .into_owned();
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
            ferry::system::reveal_path(&path)
        } else {
            ferry::system::open_path(&path)
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
    pub(super) fn start_pairing(
        &mut self,
        id: EndpointId,
        addr: iroh::EndpointAddr,
        label: String,
        keep_alias: Option<String>,
    ) {
        self.status_msg = t!("tui.pairing_connecting", name = &label).into_owned();
        let engine = self.engine.clone();
        let my_name = self.config.device_name();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let show_code = |code: &str| {
                let status = t!("tui.pairing", name = &label, code = code).into_owned();
                let _ = tx.send(UiMessage::Status(status));
            };
            let status = match request_pairing(&engine, addr.clone(), &my_name, show_code).await {
                Ok(PairOutcome::Accepted {
                    name,
                    addr: remote,
                    discovery_key,
                }) => {
                    let alias = keep_alias.unwrap_or(name);
                    let saved = Config::load().and_then(|mut c| {
                        c.add_paired(
                            id,
                            &alias,
                            remote.or(Some(addr)),
                            false,
                            Some(discovery_key),
                        )?;
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
    pub(super) fn set_clipboard(&mut self, text: &str) -> bool {
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
}
