//! `ferry history`: finished transfers, newest first.

use anyhow::{Context, Result};
use rust_i18n::t;

use super::args::HistoryFilter;
use super::display::human_size;
use ferry::history::{self, HistoryEntry, HistoryKind};
use ferry::transfer::service::Direction;

/// One line for `entry`.
fn line(entry: &HistoryEntry) -> String {
    let arrow = match entry.direction {
        Direction::Upload => "↑",
        Direction::Download => "↓",
    };
    let device = entry
        .peer_name
        .clone()
        .unwrap_or_else(|| format!("{}…", &entry.peer[..8.min(entry.peer.len())]));
    let status = match entry.status.as_str() {
        "completed" => t!("history.completed"),
        "failed" => t!("history.failed"),
        "rejected" => t!("history.rejected"),
        _ => t!("history.cancelled"),
    };
    let detail = entry
        .detail
        .as_deref()
        .map(|d| format!(" – {d}"))
        .unwrap_or_default();
    let what = match entry.kind {
        HistoryKind::File => entry.file_name.clone(),
        HistoryKind::Text => t!("history.text").into_owned(),
    };
    format!(
        "{} {arrow} {what} ({}) · {device} · {status}{detail}",
        entry
            .time
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M"),
        human_size(entry.size)
    )
}

/// `ferry history [query] [-n N] [--clear] [--send-again N]`.
///
/// # Errors
///
/// Fails when the history cannot be read or deleted, or sending again fails.
pub async fn run_history(
    query: Option<&str>,
    limit: usize,
    kind: Option<HistoryFilter>,
    clear: bool,
    send_again: Option<usize>,
) -> Result<()> {
    let path = history::default_path()?;
    if clear {
        history::clear(&path)?;
        println!("{}", t!("history.cleared"));
        return Ok(());
    }
    let mut entries = history::search(history::load(&path)?, query.unwrap_or_default());
    match kind {
        Some(HistoryFilter::Files) => entries.retain(|e| e.kind == HistoryKind::File),
        Some(HistoryFilter::Texts) => entries.retain(|e| e.kind == HistoryKind::Text),
        None => {}
    }
    if let Some(number) = send_again {
        let entry = number
            .checked_sub(1)
            .and_then(|i| entries.get(i))
            .with_context(|| t!("history.no_entry", number = number))?;
        let file = entry
            .local_file()
            .with_context(|| t!("history.no_file", number = number))?;
        let peer = entry
            .ferry_peer()
            .filter(|_| entry.direction == Direction::Upload)
            .with_context(|| t!("history.not_ferry", number = number))?;
        return super::transfer_cmd::run_send(&[file], &[peer.to_string()]).await;
    }
    if entries.is_empty() {
        println!("{}", t!("history.empty"));
    }
    for (number, entry) in entries.iter().take(limit).enumerate() {
        println!("{:>3}. {}", number + 1, line(entry));
    }
    if entries.len() > limit {
        println!("{}", t!("history.more", count = entries.len() - limit));
    }
    if ferry::config::Config::load().is_ok_and(|c| c.disable_history) {
        println!("{}", t!("history.off"));
    }
    Ok(())
}
