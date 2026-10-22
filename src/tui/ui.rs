use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::app::{App, Focus, PendingKind, Popup, display_path};
use crate::cli::display::{human_size, qr_terminal};
use crate::transfer::service::{Direction as TransferDirection, TransferStatus};

const BAR_WIDTH: usize = 16;

pub fn render(f: &mut Frame, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(f.area());
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[1]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body[0]);

    render_header(f, app, rows[0]);
    render_trusted(f, app, left[0]);
    render_nearby(f, app, left[1]);
    render_transfers(f, app, body[1]);
    render_footer(f, app, rows[2]);

    // Nakładki: okno → pole tekstowe → decyzja (najwyżej).
    if let Some(popup) = &app.popup {
        render_popup(f, app, popup);
    }
    if app.input.is_some() {
        render_input(f, app);
    }
    if !app.pending.is_empty() {
        render_pending(f, app);
    }
}

fn panel(title: String, focused: bool) -> Block<'static> {
    let color = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let lines = vec![
        Line::from(vec![
            Span::styled(
                app.config.device_name(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  ·  Node ID {}…  ·  zapis: {}",
                &app.node_id[..12],
                display_path(&app.config.download_dir())
            )),
        ]),
        Line::styled(app.status_msg.clone(), Style::default().fg(Color::Yellow)),
    ];
    let block = Block::default()
        .title(" RQS P2P Quick Share  [?] pomoc  [t] TICKET  [q] wyjście ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_trusted(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Trusted;
    let trusted = app.trusted();
    let block = panel(
        format!(" [1] Zaufane urządzenia ({}) ", trusted.len()),
        focused,
    );
    if trusted.is_empty() {
        let text = "Brak.\n[i] dodaj przez TICKET / Node ID\nalbo sparuj z „W pobliżu” ([2], [p]).";
        f.render_widget(
            Paragraph::new(text)
                .block(block)
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = trusted
        .iter()
        .map(|(id, p)| {
            let online = app.is_online(id);
            let mut spans = vec![
                Span::styled(
                    if online { "● " } else { "○ " },
                    Style::default().fg(if online {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(p.alias.clone()),
            ];
            if p.auto_accept {
                spans.push(Span::styled(
                    "  [Auto-Accept]",
                    Style::default().fg(Color::Green),
                ));
            }
            spans.push(Span::styled(
                format!("  {}…", &id[..8.min(id.len())]),
                Style::default().fg(Color::DarkGray),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight(focused))
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(app.sel_trusted));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_nearby(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Nearby;
    let block = panel(
        format!(" [2] W pobliżu ({})  [p] sparuj ", app.nearby.len()),
        focused,
    );
    if app.nearby.is_empty() {
        let mut text = vec![Line::styled(
            "Szukanie urządzeń RQS w sieci lokalnej… [r] odśwież",
            Style::default().fg(Color::DarkGray),
        )];
        if let Some(hint) = &app.discovery_hint {
            text.push(Line::raw(""));
            text.push(Line::styled(
                hint.clone(),
                Style::default().fg(Color::Yellow),
            ));
        }
        f.render_widget(
            Paragraph::new(text).block(block).wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .nearby
        .iter()
        .map(|d| {
            let mut spans = vec![
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::raw(d.name.clone()),
                Span::styled(
                    format!("  ({})", d.platform),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if let Some(p) = app.config.find_peer(&d.node_id) {
                spans.push(Span::styled(
                    format!("  ✓ sparowane jako „{}”", p.alias),
                    Style::default().fg(Color::Green),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight(focused))
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(app.sel_nearby));
    f.render_stateful_widget(list, area, &mut state);
}

fn progress_bar(pct: u64) -> String {
    let filled = (pct.min(100) as usize * BAR_WIDTH) / 100;
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

fn render_transfers(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Transfers;
    let block = panel(
        " [3] Transfery  [o] otwórz  [f] folder  [C] wyczyść ".to_string(),
        focused,
    );
    if app.transfers.is_empty() {
        f.render_widget(
            Paragraph::new(
                "Brak transferów. Wybierz urządzenie i naciśnij Enter, aby wysłać plik.",
            )
            .block(block)
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .transfers
        .iter()
        .rev()
        .map(|row| {
            let ev = &row.ev;
            let pct = ev.done.saturating_mul(100).checked_div(ev.total).unwrap_or(
                if matches!(ev.status, TransferStatus::Completed { .. }) {
                    100
                } else {
                    0
                },
            );
            let arrow = match ev.direction {
                TransferDirection::Upload => "↑",
                TransferDirection::Download => "↓",
            };
            let peer = app
                .config
                .find_peer(&ev.peer)
                .map(|p| p.alias.clone())
                .or_else(|| {
                    app.nearby
                        .iter()
                        .find(|d| d.node_id == ev.peer)
                        .map(|d| d.name.clone())
                })
                .unwrap_or_else(|| format!("{}…", &ev.peer[..8.min(ev.peer.len())]));
            let (detail, color) = match &ev.status {
                TransferStatus::Preparing => ("liczenie haszu BLAKE3…".to_string(), Color::Blue),
                TransferStatus::AwaitingDecision => {
                    ("oczekiwanie na akceptację…".to_string(), Color::Yellow)
                }
                TransferStatus::Running => {
                    let speed = if row.speed > 0.0 {
                        format!(" · {}/s", human_size(row.speed as u64))
                    } else {
                        String::new()
                    };
                    (
                        format!("{} / {}{speed}", human_size(ev.done), human_size(ev.total)),
                        Color::Yellow,
                    )
                }
                TransferStatus::Completed { path } => (
                    match path {
                        Some(p) => format!(
                            "{} · zakończono ✓ · {}",
                            human_size(ev.total),
                            display_path(std::path::Path::new(p))
                        ),
                        None => format!("{} · dostarczono ✓", human_size(ev.total)),
                    },
                    Color::Green,
                ),
                TransferStatus::Rejected { reason } => (format!("odrzucono: {reason}"), Color::Red),
                TransferStatus::Failed { error } => (format!("błąd: {error}"), Color::Red),
            };
            let line1 = Line::from(vec![
                Span::raw(format!("{arrow} {}  ", ev.file_name)),
                Span::styled(progress_bar(pct), Style::default().fg(color)),
                Span::raw(format!(" {pct:>3}%")),
                Span::styled(format!("  {peer}"), Style::default().fg(Color::DarkGray)),
            ]);
            let line2 = Line::styled(format!("   {detail}"), Style::default().fg(color));
            ListItem::new(Text::from(vec![line1, line2]))
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight(focused))
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(app.sel_transfer));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let hints = match app.focus {
        Focus::Trusted => {
            "Enter wyślij plik · e nazwa · a Auto-Accept · x usuń · i dodaj · Tab panel · g katalog · N nazwa urządzenia"
        }
        Focus::Nearby => {
            "p sparuj · Enter wyślij plik · r szukaj · i dodaj przez TICKET · Tab panel"
        }
        Focus::Transfers => {
            "Enter/o otwórz plik · f otwórz folder · C wyczyść zakończone · Tab panel"
        }
    };
    f.render_widget(
        Paragraph::new(hints).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

/// Prostokąt na środku ekranu, przycięty do dostępnej przestrzeni.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn render_pending(f: &mut Frame, app: &App) {
    let Some(p) = app.pending.front() else {
        return;
    };
    let queue = if app.pending.len() > 1 {
        format!(" (w kolejce: {})", app.pending.len() - 1)
    } else {
        String::new()
    };
    let (title, lines) = match &p.kind {
        PendingKind::File {
            peer_label,
            file_name,
            file_size,
        } => (
            format!(" Przychodzący plik{queue} "),
            vec![
                Line::styled(
                    file_name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::raw(format!("{} · od: {peer_label}", human_size(*file_size))),
                Line::raw(""),
                Line::styled(
                    "[y] odbierz    [n] odrzuć",
                    Style::default().fg(Color::Yellow),
                ),
            ],
        ),
        PendingKind::Pair { name, code } => (
            format!(" Prośba o sparowanie{queue} "),
            vec![
                Line::raw(format!("„{name}” chce dodać to urządzenie do zaufanych.")),
                Line::raw("Sparuj tylko, gdy drugie urządzenie pokazuje ten sam kod:"),
                Line::raw(""),
                Line::styled(
                    code.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                Line::styled(
                    "[y] sparuj    [n] odrzuć",
                    Style::default().fg(Color::Yellow),
                ),
            ],
        ),
    };
    let area = centered(f.area(), 64, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_input(f: &mut Frame, app: &App) {
    let Some(input) = &app.input else {
        return;
    };
    let mut lines = vec![
        Line::styled(input.hint.clone(), Style::default().fg(Color::DarkGray)),
        Line::raw(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::raw(input.value.clone()),
            Span::styled("█", Style::default().fg(Color::Cyan)),
        ]),
    ];
    if let Some(err) = &input.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(err.clone(), Style::default().fg(Color::Red)));
    }
    lines.push(Line::raw(""));
    let mut keys = "Enter zatwierdź · Esc anuluj · Ctrl+U wyczyść".to_string();
    if input.kind.is_path() {
        keys.push_str(" · Tab podpowiedź");
    }
    lines.push(Line::styled(keys, Style::default().fg(Color::DarkGray)));

    let width = f.area().width.saturating_sub(8).clamp(40, 100);
    // Wysokość z zapasem na zawinięcie długich wartości (np. TICKET).
    let wrapped = (input.value.chars().count() as u16 + 4) / width.saturating_sub(4).max(1);
    let area = centered(f.area(), width, lines.len() as u16 + 2 + wrapped);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(input.title.clone())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_popup(f: &mut Frame, app: &App, popup: &Popup) {
    match popup {
        Popup::Ticket => render_ticket(f, app),
        Popup::Help => render_help(f),
        Popup::ConfirmRemove { alias, .. } => {
            let area = centered(f.area(), 56, 4);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(vec![
                    Line::raw(format!("Usunąć „{alias}” z zaufanych urządzeń?")),
                    Line::styled(
                        "[y] usuń    dowolny klawisz – anuluj",
                        Style::default().fg(Color::Yellow),
                    ),
                ])
                .block(
                    Block::default()
                        .title(" Usuń urządzenie ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Red)),
                ),
                area,
            );
        }
    }
}

fn render_ticket(f: &mut Frame, app: &App) {
    let screen = f.area();
    let qr = qr_terminal(&app.ticket_str);
    let qr_lines: Vec<&str> = qr.lines().collect();
    let qr_width = qr_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let width = screen
        .width
        .saturating_sub(4)
        .clamp(40, qr_width.max(90) + 4);
    let ticket_rows = (app.ticket_str.len() as u16).div_ceil(width.saturating_sub(4).max(1));
    // Kod QR tylko wtedy, gdy zmieści się w oknie terminala.
    let show_qr =
        qr_width + 4 <= screen.width && qr_lines.len() as u16 + ticket_rows + 8 <= screen.height;

    let mut lines: Vec<Line> = Vec::new();
    if show_qr {
        lines.extend(qr_lines.iter().map(|l| Line::raw(l.to_string())));
    } else {
        lines.push(Line::styled(
            "Powiększ okno terminala, aby zobaczyć kod QR.",
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        app.ticket_str.clone(),
        Style::default().fg(Color::Cyan),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "[c] kopiuj do schowka · dowolny klawisz – zamknij",
        Style::default().fg(Color::Yellow),
    ));

    let height = lines.len() as u16 + ticket_rows + 2;
    let area = centered(screen, width, height);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" TICKET tego urządzenia – zeskanuj lub wklej w „Dodaj urządzenie” ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_help(f: &mut Frame) {
    let rows = [
        (
            "Tab / 1 2 3",
            "przełącz panel (zaufane / w pobliżu / transfery)",
        ),
        ("↑ ↓", "wybór na liście"),
        (
            "Enter / s",
            "wyślij plik do wybranego urządzenia / otwórz transfer",
        ),
        ("p", "sparuj z urządzeniem w pobliżu"),
        (
            "i / +",
            "dodaj urządzenie przez TICKET lub Node ID (+ prośba o sparowanie)",
        ),
        (
            "e / a / x",
            "zmień nazwę / przełącz Auto-Accept / usuń urządzenie",
        ),
        ("o / f", "otwórz odebrany plik / jego folder"),
        ("C", "usuń zakończone transfery z listy"),
        ("t / c", "pokaż TICKET z kodem QR / kopiuj TICKET"),
        ("g / N", "katalog zapisu / nazwa tego urządzenia"),
        ("r", "szukaj urządzeń w sieci lokalnej"),
        ("y / n", "odpowiedź na ofertę pliku lub prośbę o sparowanie"),
        ("q / Esc", "wyjście"),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:<14}"), Style::default().fg(Color::Cyan)),
                Span::raw(*d),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "W polu tekstowym: Tab podpowiada ścieżkę; plik można przeciągnąć do okna terminala.",
        Style::default().fg(Color::DarkGray),
    ));
    let area = centered(f.area(), 92, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Pomoc – dowolny klawisz zamyka ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}
