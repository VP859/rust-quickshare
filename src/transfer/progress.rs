use indicatif::{ProgressBar, ProgressStyle};

pub fn create_progress_bar(total_bytes: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})")
            .expect("Błąd tworzenia szablonu paska postępu")
            .progress_chars("#>-"),
    );
    pb.set_message(message.to_string());
    pb
}
