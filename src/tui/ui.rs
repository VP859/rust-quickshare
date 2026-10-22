use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
};

use super::app::{App, PendingKind};
use crate::transfer::service::{Direction as TransferDirection, TransferStatus};

pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(9),
        ])
        .split(f.area());

    render_header(f, app, chunks[0]);
    render_transfers(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let text = format!(
        " Node ID: {}\n TICKET : {}\n Status : {}",
        app.node_id, app.ticket_str, app.status_msg
    );
    let block = Block::default()
        .title(" RQS P2P [↑↓ wybór | p: sparuj | r: szukaj | c: kopiuj TICKET | q: wyjście] ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn render_transfers(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Aktywne i Historyczne Transfery Plików ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    if app.transfers.is_empty() {
        let p = Paragraph::new(" Brak aktywnych transferów.").block(block);
        f.render_widget(p, area);
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Najnowsze transfery na górze; tyle, ile zmieści się w panelu.
    let visible = (inner.height / 2).max(1) as usize;
    let items: Vec<_> = app.transfers.iter().rev().take(visible).collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(2); items.len()])
        .split(inner);

    for (item, row) in items.into_iter().zip(rows.iter()) {
        let pct = if item.total > 0 {
            (item.done as f64 / item.total as f64 * 100.0) as u16
        } else {
            0
        };
        let arrow = match item.direction {
            TransferDirection::Upload => "↑",
            TransferDirection::Download => "↓",
        };
        let (state, color) = match &item.status {
            TransferStatus::Preparing => ("haszowanie".to_string(), Color::Blue),
            TransferStatus::AwaitingDecision => ("oczekuje".to_string(), Color::Yellow),
            TransferStatus::Running => (format!("{pct}%"), Color::Yellow),
            TransferStatus::Completed { .. } => ("zakończono ✓".to_string(), Color::Green),
            TransferStatus::Rejected { reason } => (format!("odrzucono: {reason}"), Color::Red),
            TransferStatus::Failed { error } => (format!("błąd: {error}"), Color::Red),
        };
        let label = format!(
            "{arrow} {} — {:.1}/{:.1} MB — {state}",
            item.file_name,
            item.done as f64 / 1_048_576.0,
            item.total as f64 / 1_048_576.0,
        );

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::BOTTOM))
            .gauge_style(Style::default().fg(color).bg(Color::Black))
            .percent(pct.min(100))
            .label(label);

        f.render_widget(gauge, *row);
    }
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(35),
            Constraint::Percentage(35),
        ])
        .split(area);

    let peer_items: Vec<ListItem> = app
        .config
        .peers
        .iter()
        .map(|(id, info)| {
            let auto = if info.auto_accept {
                "[Auto-Accept]"
            } else {
                ""
            };
            ListItem::new(format!(
                "• {} ({}) {}",
                info.alias,
                &id[..8.min(id.len())],
                auto
            ))
        })
        .collect();

    let peers_list = List::new(peer_items).block(
        Block::default()
            .title(" Zaufane Urządzenia ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
    );
    f.render_widget(peers_list, chunks[0]);

    let nearby_items: Vec<ListItem> = if app.nearby.is_empty() {
        vec![ListItem::new("Szukanie… [r] odśwież")]
    } else {
        app.nearby
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let paired = app.config.find_peer(&d.node_id).is_some();
                let marker = if i == app.selected { "▶" } else { " " };
                let state = if paired { " ✓" } else { "" };
                let style = if i == app.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{marker} {} ({}){state}", d.name, d.platform)).style(style)
            })
            .collect()
    };
    let nearby_list = List::new(nearby_items).block(
        Block::default()
            .title(" W pobliżu [p: sparuj] ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    f.render_widget(nearby_list, chunks[1]);

    let prompt_text = match app.pending.front().map(|p| &p.kind) {
        Some(PendingKind::File {
            peer_label,
            file_name,
            file_size,
        }) => format!(
            "Oferta pliku (w kolejce: {})\nPlik: {}\nRozmiar: {:.2} MB\nOd: {}\n\nAkceptujesz? [y / n]",
            app.pending.len(),
            file_name,
            *file_size as f64 / 1_048_576.0,
            peer_label
        ),
        Some(PendingKind::Pair { name, code }) => format!(
            "Prośba o sparowanie (w kolejce: {})\nUrządzenie: {}\nKod: {}\n\nCzy kod zgadza się z drugim\nurządzeniem? [y / n]",
            app.pending.len(),
            name,
            code
        ),
        None => "Brak oczekujących decyzji.".to_string(),
    };

    let prompt = Paragraph::new(prompt_text).block(
        Block::default()
            .title(" Oczekujące Decyzje [y/n] ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(prompt, chunks[2]);
}
