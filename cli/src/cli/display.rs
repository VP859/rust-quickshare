//! Terminal output helpers: sizes, progress bars and QR codes.

use indicatif::{ProgressBar, ProgressStyle};
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use rust_i18n::t;
use std::sync::Mutex;

use ferry::transfer::service::{Direction, TransferEvent, TransferStatus};

pub use ferry::size::human_size;

/// Creates a byte progress bar with the given message.
pub fn create_progress_bar(total_bytes: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(total_bytes);
    if let Ok(style) = ProgressStyle::default_bar().template(
        "{msg}{prefix}\n[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
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
        if let Some(route) = ev.route {
            pb.set_prefix(format!(" · {}", route.label()));
        }
        match ev.status {
            TransferStatus::Preparing => pb.set_message(t!("cli.hashing", name = &ev.file_name)),
            TransferStatus::AwaitingDecision => pb.set_message(t!("cli.awaiting")),
            TransferStatus::Running => pb.set_position(ev.done),
            TransferStatus::Retrying { attempt, of } => {
                pb.set_prefix(format!(
                    " · {}",
                    t!("cli.retrying", attempt = attempt, of = of)
                ));
            }
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
            TransferStatus::Cancelled => {
                finish(pb, t!("cli.cancelled", name = &ev.file_name).into_owned())
            }
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
