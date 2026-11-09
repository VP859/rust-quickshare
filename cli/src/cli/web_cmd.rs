//! `ferry web`: share files with, and receive files from, a web browser on
//! the local network (a phone or computer without Ferry).

use anyhow::Result;
use iroh::EndpointId;
use rust_i18n::t;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::display::{TerminalProgress, human_size, qr_terminal};
use ferry::config::Config;
use ferry::protocol::messages::TransferOffer;
use ferry::transfer::service::{OfferDecision, TransferEvent, TransferHandler};
use ferry::web::{BrowserOffer, WebShare, WebShareOptions, firewall_hint};

/// Asks in the terminal about files sent from the browser.
struct WebHandler {
    /// Folder for received files.
    dest_dir: PathBuf,
    /// Name shown on the page.
    name: String,
    /// Standard input is a terminal, so questions can be answered.
    interactive: bool,
    /// Progress bars of the transfers.
    progress: TerminalProgress,
    /// One question at a time.
    prompt: tokio::sync::Mutex<()>,
}

impl TransferHandler for WebHandler {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Reject {
            reason: t!("transfer.user_rejected").into_owned(),
        }
    }

    fn on_event(&self, event: TransferEvent) {
        self.progress.handle(event);
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }

    async fn decide_browser(&self, offer: &BrowserOffer) -> OfferDecision {
        let reject = || OfferDecision::Reject {
            reason: t!("transfer.user_rejected").into_owned(),
        };
        if !self.interactive {
            return reject();
        }
        let _one = self.prompt.lock().await;
        let warning = offer
            .risk
            .map_or_else(String::new, |r| format!("{}\n", r.warning()));
        let question = format!(
            "\n{warning}{} [y/N]: ",
            t!(
                "cli.web_offer",
                from = offer.label(),
                count = offer.count,
                size = human_size(offer.total_size),
                names = offer.names_preview()
            )
        );
        let yes = tokio::task::spawn_blocking(move || {
            use std::io::Write;
            print!("{question}");
            let _ = std::io::stdout().flush();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).is_ok()
                && answer.trim().eq_ignore_ascii_case("y")
        })
        .await
        .unwrap_or(false);
        if yes {
            OfferDecision::Accept {
                dest_dir: self.dest_dir.clone(),
            }
        } else {
            reject()
        }
    }
}

/// `ferry web [files] [--no-receive] [--minutes N]`.
///
/// # Errors
///
/// Fails when a path is not a file or no port can be opened.
pub async fn run_web(files: Vec<PathBuf>, no_receive: bool, minutes: u64) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let minutes = minutes.clamp(1, 24 * 60);
    let handler = Arc::new(WebHandler {
        dest_dir: config.download_dir(),
        name: config.device_name(),
        interactive: std::io::stdin().is_terminal(),
        progress: TerminalProgress::default(),
        prompt: tokio::sync::Mutex::new(()),
    });
    let receive = !no_receive;
    let options = WebShareOptions {
        files,
        receive,
        lifetime: Some(Duration::from_secs(minutes * 60)),
        ..WebShareOptions::default()
    };
    let mut share = WebShare::start(None, options, handler).await?;

    println!("\n==================================================================");
    println!(" {}", t!("cli.web_title", minutes = minutes));
    println!("==================================================================");
    println!("{}", qr_terminal(share.url()));
    for url in share.urls() {
        println!(" {url}");
    }
    println!();
    if receive {
        println!(
            " {}",
            t!("cli.web_receive", path = config.download_dir().display())
        );
    }
    println!(" {}", t!("cli.web_security"));
    println!(" {}", t!("cli.web_phone_help"));
    let port = share.port();
    if let Ok(Some(hint)) = tokio::task::spawn_blocking(move || firewall_hint(port)).await {
        println!(" {hint}");
    }
    println!(" {}\n", t!("cli.web_stop"));

    tokio::select! {
        () = share.finished() => println!("\n{}", t!("cli.web_expired")),
        _ = tokio::signal::ctrl_c() => {
            share.stop();
            println!("\n{}", t!("cli.web_stopped"));
        }
    }
    Ok(())
}
