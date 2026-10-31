//! Answers to questions and submitted text fields.

use super::*;

impl App {
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

    /// Address of a device; see [`ferry::discovery::resolve_target`].
    fn target_addr(&self, node_id: &str) -> Result<iroh::EndpointAddr, String> {
        ferry::discovery::resolve_target(&self.config, self.discovery.as_ref(), node_id)
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
        if let Some(bad) = paths.iter().find(|p| !p.is_file() && !p.is_dir()) {
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
