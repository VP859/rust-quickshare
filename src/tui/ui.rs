use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
};

use super::app::App;

pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(8),
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
        .title(" RQS P2P Node Status [c: Kopiuj Ticket | q: Wyjście] ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn render_transfers(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Aktywne i Historiowe Transfery Plików ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    if app.transfers.is_empty() {
        let p = Paragraph::new(" Brak aktywnych transferów.").block(block);
        f.render_widget(p, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(3); app.transfers.len()])
        .split(area);

    for (i, (_, item)) in app.transfers.iter().enumerate() {
        if i >= chunks.len() {
            break;
        }
        let pct = if item.file_size > 0 {
            (item.bytes_transferred as f64 / item.file_size as f64 * 100.0) as u16
        } else {
            0
        };

        let label = format!(
            "{} ({:.1} MB/s) - {}/{} B",
            item.file_name, item.speed_mbps, item.bytes_transferred, item.file_size
        );

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::BOTTOM))
            .gauge_style(Style::default().fg(Color::Yellow).bg(Color::Black))
            .percent(pct.min(100))
            .label(label);

        f.render_widget(gauge, chunks[i]);
    }
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
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

    let prompt_text = if let Some((node, name, size)) = &app.pending_offer {
        format!(
            "Otrzymano ofertę pliku!\nPlik: {}\nRozmiar: {:.2} MB\nOd: {}\n\nCzy akceptujesz? [y / n]",
            name,
            *size as f64 / 1_048_576.0,
            node
        )
    } else {
        "Brak oczekujących prośb transferu.".to_string()
    };

    let prompt = Paragraph::new(prompt_text).block(
        Block::default()
            .title(" Oczekujące Decyzje [y/n] ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(prompt, chunks[1]);
}
