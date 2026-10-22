use indicatif::{ProgressBar, ProgressStyle};
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use std::sync::Mutex;

use crate::transfer::service::{Direction, TransferEvent, TransferStatus};

/// Rozmiar w czytelnych jednostkach: `512 B`, `12.3 KB`, `4.0 MB`, `1.25 GB`.
pub fn human_size(bytes: u64) -> String {
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

/// Kod QR TICKETU do wyświetlenia w terminalu (pusty, gdy tekst się nie mieści).
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

pub fn print_ticket(title: &str, ticket: &str) {
    println!("\n==================================================================");
    println!(" {title}");
    println!("==================================================================");
    println!("{}", qr_terminal(ticket));
    println!(" {ticket}\n");
}

/// Renderuje zdarzenia transferów jako paski postępu (po jednym na plik).
#[derive(Default)]
pub struct TerminalProgress {
    current: Mutex<Option<(String, ProgressBar)>>,
}

impl TerminalProgress {
    pub fn handle(&self, ev: TransferEvent) {
        let Ok(mut current) = self.current.lock() else {
            return;
        };
        if current.as_ref().map(|(id, _)| id != &ev.id).unwrap_or(true) {
            if let Some((_, old)) = current.take() {
                old.abandon();
            }
            let verb = match ev.direction {
                Direction::Upload => "Wysyłanie",
                Direction::Download => "Pobieranie",
            };
            *current = Some((
                ev.id.clone(),
                create_progress_bar(ev.total, &format!("{verb}: {}", ev.file_name)),
            ));
        }
        let Some((_, pb)) = current.as_ref() else {
            return;
        };
        if ev.total > 0 {
            pb.set_length(ev.total);
        }
        match ev.status {
            TransferStatus::Preparing => pb.set_message(format!("Hasz BLAKE3: {}", ev.file_name)),
            TransferStatus::AwaitingDecision => pb.set_message("Oczekiwanie na akceptację..."),
            TransferStatus::Running => pb.set_position(ev.done),
            TransferStatus::Completed { path } => {
                pb.set_position(ev.total);
                finish(
                    pb,
                    match path {
                        Some(p) => format!("[SUKCES] Zapisano i zweryfikowano: {p}"),
                        None => format!("[SUKCES] Dostarczono i zweryfikowano: {}", ev.file_name),
                    },
                );
            }
            TransferStatus::Rejected { reason } => {
                finish(pb, format!("[ODRZUCONO] {}: {reason}", ev.file_name))
            }
            TransferStatus::Failed { error } => {
                finish(pb, format!("[BŁĄD] {}: {error}", ev.file_name))
            }
        }
    }
}

/// Komunikat końcowy; gdy pasek jest ukryty (wyjście nie jest terminalem) – zwykły wydruk.
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
