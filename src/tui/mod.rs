pub mod app;
pub mod ui;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use app::{App, TuiHandler, UiMessage};

use crate::transfer::service;

pub async fn run_tui() -> Result<()> {
    // Węzeł startujemy przed przełączeniem terminala, aby błędy były czytelne.
    let app = App::new().await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let handler = Arc::new(TuiHandler {
        tx,
        dest_dir: app.config.download_dir(),
    });
    let server = tokio::spawn(service::serve(app.engine.clone(), handler));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let engine = app.engine.clone();
    let res = run_app(&mut terminal, app, rx).await;

    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    server.abort();
    engine.close().await;
    res
}

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
        terminal.draw(|f| ui::render(f, &app))?;

        // Odczyt klawiatury blokuje – wykonujemy go poza wątkami runtime'u.
        let key = tokio::task::spawn_blocking(|| -> io::Result<Option<event::KeyEvent>> {
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                return Ok(Some(key));
            }
            Ok(None)
        })
        .await??;

        if let Some(key) = key {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    while !app.pending.is_empty() {
                        app.answer_pending_offer(false);
                    }
                    return Ok(());
                }
                KeyCode::Char('c') => app.copy_ticket_to_clipboard(),
                KeyCode::Char('y') => app.answer_pending_offer(true),
                KeyCode::Char('n') => app.answer_pending_offer(false),
                _ => {}
            }
        }
    }
}
