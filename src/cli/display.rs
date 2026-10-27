//! Terminal output helpers: sizes, progress bars and QR codes.

use indicatif::{ProgressBar, ProgressStyle};
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use rust_i18n::t;
use std::sync::Mutex;

use crate::transfer::service::{Direction, TransferEvent, TransferStatus};

/// Formats a size with a readable unit: `512 B`, `12.3 KB`, `4.0 MB`, `1.25 GB`.
///
/// # Examples
///
/// ```
/// use rqs_p2p::cli::display::human_size;
///
/// assert_eq!(human_size(18), "18 B");
/// assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
/// ```
pub fn human_size(bytes: u64) -> String {
    /// Units for successive powers of 1024.
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let decimals = if unit >= 3 { 2 } else { 1 };
    format!("{value:.decimals$} {}", UNITS[unit])
}

/// Creates a byte progress bar with the given message.
pub fn create_progress_bar(total_bytes: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(total_bytes);
    if let Ok(style) = ProgressStyle::default_bar().template(
        "{msg}\n[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
    ) {
        pb.set_style(style.progress_chars("#>-"));
    }
    pb.set_message(message.to_string());
    pb
}

/// Renders `text` as a QR code for a terminal; empty if the text does not fit.
pub fn qr_terminal(text: &str) -> String {
    QrCode::new(text.as_bytes())
        .map(|code| {
            code.render::<Dense1x2>()
                .dark_color(Dense1x2::Light)
                .light_color(Dense1x2::Dark)
                .quiet_zone(true)
                .build()
        })
        .unwrap_or_default()
}

/// Prints a TICKET with a title and its QR code.
pub fn print_ticket(title: &str, ticket: &str) {
    println!("\n==================================================================");
    println!(" {title}");
    println!("==================================================================");
    println!("{}", qr_terminal(ticket));
    println!(" {ticket}\n");
}

/// Renders [`TransferEvent`]s as progress bars, one per file.
#[derive(Default)]
pub struct TerminalProgress {
    /// Transfer ID and bar of the transfer shown last.
    current: Mutex<Option<(String, ProgressBar)>>,
}

impl TerminalProgress {
    /// Updates the bar of the event's transfer, starting a new bar for a new transfer.
    pub fn handle(&self, ev: TransferEvent) {
        let Ok(mut current) = self.current.lock() else {
            return;
        };
        if current.as_ref().map(|(id, _)| id != &ev.id).unwrap_or(true) {
            if let Some((_, old)) = current.take() {
                old.abandon();
            }
            let label = match ev.direction {
                Direction::Upload => t!("cli.sending", name = &ev.file_name),
                Direction::Download => t!("cli.receiving", name = &ev.file_name),
            };
            *current = Some((ev.id.clone(), create_progress_bar(ev.total, &label)));
        }
        let Some((_, pb)) = current.as_ref() else {
            return;
        };
        if ev.total > 0 {
            pb.set_length(ev.total);
        }
        match ev.status {
            TransferStatus::Preparing => pb.set_message(t!("cli.hashing", name = &ev.file_name)),
            TransferStatus::AwaitingDecision => pb.set_message(t!("cli.awaiting")),
            TransferStatus::Running => pb.set_position(ev.done),
            TransferStatus::Completed { path } => {
                pb.set_position(ev.total);
                finish(
                    pb,
                    match path {
                        Some(p) => t!("cli.saved", path = p).into_owned(),
                        None => t!("cli.delivered", name = &ev.file_name).into_owned(),
                    },
                );
            }
            TransferStatus::Rejected { reason } => finish(
                pb,
                t!("cli.rejected", name = &ev.file_name, reason = reason).into_owned(),
            ),
            TransferStatus::Failed { error } => finish(
                pb,
                t!("cli.failed", name = &ev.file_name, error = error).into_owned(),
            ),
        }
    }
}

/// Prints the final message; when output is not a terminal the bar is hidden,
/// so the message is printed explicitly.
fn finish(pb: &ProgressBar, msg: String) {
    if pb.is_hidden() {
        println!("{msg}");
    }
    pb.finish_with_message(msg);
}

#[cfg(test)]
mod tests {
    use super::human_size;

    #[test]
    fn sizes_use_readable_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(812), "812 B");
        assert_eq!(human_size(12 * 1024 + 300), "12.3 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024 / 2), "1.50 GB");
    }
}
