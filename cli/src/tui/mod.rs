//! `ferry tui`: interactive terminal user interface built with ratatui.
//!
//! Three panels (trusted devices, nearby devices, transfers) plus dialogs for
//! decisions, text input, the TICKET with its QR code and help. Press `?` in
//! the interface for the list of keyboard shortcuts.

pub mod app;
pub mod ui;

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rust_i18n::t;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use app::{App, Focus, Popup, TuiHandler, UiMessage};

use ferry::transfer::service;

/// Runs the TUI until the user quits.
///
/// The node starts before the terminal switches to the alternate screen, so
/// startup errors stay readable.
///
/// # Errors
///
/// Fails when the node cannot start or the terminal cannot be controlled.
pub async fn run_tui() -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    let app = App::new(tx.clone()).await?;
    let nearby_task = app.discovery.as_ref().map(|d| {
        let mut watch = d.subscribe();
        let tx = tx.clone();
        tokio::spawn(async move {
            while watch.changed().await.is_ok() {
                let devices = watch.borrow_and_update().clone();
                if tx.send(UiMessage::Nearby(devices)).is_err() {
                    break;
                }
            }
        })
    });
    let handler = Arc::new(TuiHandler { tx });
    // LocalSend compatibility mode, when enabled; stops with the TUI.
    let _localsend = if app.config.localsend {
        ferry::localsend::LocalSend::start(
            &app.engine,
            ferry::localsend::LocalSendOptions::from_config(&app.config),
            handler.clone(),
        )
        .await
        .inspect_err(|e| tracing::warn!("{e:#}"))
        .ok()
    } else {
        None
    };
    let server = tokio::spawn(service::serve(app.engine.clone(), handler));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let engine = app.engine.clone();
    let res = run_app(&mut terminal, app, rx).await;

    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    server.abort();
    if let Some(task) = nearby_task {
        task.abort();
    }
    engine.close().await;
    res
}

/// Main loop: applies background messages, draws a frame and handles one key.
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    mut app: App,
    mut rx: mpsc::UnboundedReceiver<UiMessage>,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    loop {
        while let Ok(msg) = rx.try_recv() {
            app.handle_message(msg);
        }
        app.tick();
        terminal.draw(|f| ui::render(f, &app))?;

        let event = tokio::task::spawn_blocking(|| -> io::Result<Option<Event>> {
            if event::poll(Duration::from_millis(100))? {
                return Ok(Some(event::read()?));
            }
            Ok(None)
        })
        .await??;

        match event {
            Some(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if handle_key(&mut app, key) {
                    while !app.pending.is_empty() {
                        app.answer_pending(false);
                    }
                    return Ok(());
                }
            }
            Some(Event::Paste(text)) => {
                if let Some(input) = app.input.as_mut() {
                    input
                        .value
                        .push_str(text.replace(['\r', '\n'], " ").trim_end());
                    input.error = None;
                }
            }
            _ => {}
        }
    }
}

/// Handles a key press; returns `true` when the user quits.
///
/// Priority: pending decision, then the input dialog, then an informational
/// dialog, then the panels.
fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if !app.pending.is_empty() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.answer_pending(true),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.answer_pending(false),
            _ => {}
        }
        return false;
    }

    if let Some(input) = app.input.as_mut() {
        match key.code {
            KeyCode::Esc => app.input = None,
            KeyCode::Enter => app.submit_input(),
            KeyCode::Tab => app.input_complete(),
            KeyCode::Backspace => {
                input.value.pop();
                input.error = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.value.clear();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.value.push(c);
                input.error = None;
            }
            _ => {}
        }
        return false;
    }

    if let Some(popup) = app.popup.take() {
        match popup {
            Popup::ConfirmRemove { node_id, .. } => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    app.confirm_remove(&node_id);
                }
            }
            Popup::Ticket => {
                if key.code == KeyCode::Char('c') {
                    app.copy_ticket_to_clipboard();
                    app.popup = Some(Popup::Ticket);
                }
            }
            Popup::SafetyNumber {
                node_id, verified, ..
            } => {
                // `m` toggles the mark, anything else just closes the window.
                if key.code == KeyCode::Char('m') {
                    app.set_verified(&node_id, !verified);
                }
            }
            Popup::Help => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Tab => app.cycle_focus(false),
        KeyCode::BackTab => app.cycle_focus(true),
        KeyCode::Up | KeyCode::Char('k') => app.select(-1),
        KeyCode::Down | KeyCode::Char('j') => app.select(1),
        KeyCode::Char('1') => app.focus = Focus::Trusted,
        KeyCode::Char('2') => app.focus = Focus::Nearby,
        KeyCode::Char('3') => app.focus = Focus::Transfers,
        KeyCode::Enter | KeyCode::Char('s') => app.activate(),
        KeyCode::Char('T') => app.ask_send_text(),
        KeyCode::Char('o') => app.open_selected_transfer(false),
        KeyCode::Char('f') => app.open_selected_transfer(true),
        KeyCode::Char('C') => app.clear_finished(),
        KeyCode::Char('p') if app.focus == Focus::Transfers => app.pause_selected_transfer(),
        KeyCode::Char('p') => {
            if app.focus == Focus::Nearby {
                app.pair_selected();
            } else {
                app.status_msg = t!("tui.pair_where").into_owned();
            }
        }
        KeyCode::Char('e') => app.open_rename(),
        KeyCode::Char('a') => app.toggle_auto_accept(),
        KeyCode::Delete | KeyCode::Char('x') if app.focus == Focus::Transfers => {
            app.cancel_selected_transfer();
        }
        KeyCode::Delete | KeyCode::Char('x') => app.ask_remove(),
        KeyCode::Char('i') | KeyCode::Char('+') => app.open_add_device(),
        KeyCode::Char('g') => app.open_download_dir(),
        KeyCode::Char('N') => app.open_device_name(),
        KeyCode::Char('L') => app.cycle_language(),
        KeyCode::Char('M') => app.toggle_network_mode(),
        KeyCode::Char('H') => app.toggle_hidden(),
        KeyCode::Char('r') => app.refresh_nearby(),
        KeyCode::Char('v') => app.show_safety_number(),
        KeyCode::Char('t') => app.popup = Some(Popup::Ticket),
        KeyCode::Char('c') => app.copy_ticket_to_clipboard(),
        KeyCode::Char('?') | KeyCode::F(1) => app.popup = Some(Popup::Help),
        _ => {}
    }
    false
}
