//! `ferry localsend scan`, `send` and `text`: sending to LocalSend apps.

use anyhow::{Context, Result};
use rust_i18n::t;
use std::io::IsTerminal;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use super::args::LocalsendCommands;
use super::display::TerminalProgress;
use ferry::config::Config;
use ferry::localsend::client::{self, LocalSendDevice, OwnDevice, SendResult};
use ferry::node::NodeEngine;

/// How long `send` looks for a device given by name.
const LOOKUP: Duration = Duration::from_secs(3);

/// Runs a `ferry localsend` subcommand.
///
/// # Errors
///
/// Fails when the network cannot be used, the device is not found or the
/// transfer fails.
pub async fn run(command: LocalsendCommands) -> Result<()> {
    match command {
        LocalsendCommands::Scan { timeout } => {
            scan(Duration::from_secs(timeout.clamp(1, 60))).await
        }
        LocalsendCommands::Send { files, to, pin } => send(&files, &to, pin.as_deref()).await,
        LocalsendCommands::Text { text, to, pin } => send_text(text, &to, pin.as_deref()).await,
    }
}

/// This device as LocalSend apps see it.
fn me() -> OwnDevice {
    OwnDevice::named(&Config::load().unwrap_or_default().device_name())
}

/// Announces this device and lists the LocalSend apps that answered.
async fn scan(wait: Duration) -> Result<()> {
    let engine = NodeEngine::with_persistent_identity().await?;
    println!("{}", t!("localsend.scanning"));
    let found = client::scan(&engine, &me(), wait).await;
    engine.close().await;
    let found = found?;
    if found.is_empty() {
        println!("{}", t!("localsend.none_found"));
        return Ok(());
    }
    for (i, device) in found.iter().enumerate() {
        println!(
            " {:<3} {:<32} {:<22} {}",
            i + 1,
            device.label(),
            device.address,
            if device.https { "https" } else { "http" }
        );
    }
    println!("\n{}", t!("localsend.send_hint"));
    Ok(())
}

/// The device `to`: an address is asked directly, a name is looked up on the
/// local network.
async fn resolve(to: &str) -> Result<LocalSendDevice> {
    if let Ok(ip) = to.parse::<IpAddr>() {
        return LocalSendDevice::probe(ip, None, &me()).await;
    }
    if let Ok(addr) = to.parse::<SocketAddr>() {
        return LocalSendDevice::probe(addr.ip(), Some(addr.port()), &me()).await;
    }
    if let Some(device) = client::find(to) {
        return Ok(device);
    }
    let engine = NodeEngine::with_persistent_identity().await?;
    let scanned = client::scan(&engine, &me(), LOOKUP).await;
    engine.close().await;
    scanned?;
    client::find(to).with_context(|| t!("localsend.not_found", name = to))
}

/// Reads a PIN typed in the terminal.
fn ask_pin(name: &str) -> Result<String> {
    use std::io::Write;
    print!("{}", t!("localsend.pin_prompt", name = name));
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let pin = line.trim().to_string();
    anyhow::ensure!(!pin.is_empty(), t!("localsend.pin_required"));
    Ok(pin)
}

/// Sends `files` to the LocalSend app `to`.
async fn send(files: &[PathBuf], to: &str, pin: Option<&str>) -> Result<()> {
    let device = resolve(to).await?;
    println!("{}", t!("localsend.unverified", name = device.label()));
    println!("{}", t!("localsend.waiting", name = device.label()));
    let mut pin = pin.map(str::to_string);
    let result = loop {
        let progress = TerminalProgress::default();
        let sent = client::send(&device, files, &me(), pin.as_deref(), &|e| {
            progress.handle(e)
        })
        .await;
        match sent {
            // Ask for the PIN in a terminal, as often as the app refuses it.
            Err(e) if e.is::<client::PinRequired>() && std::io::stdin().is_terminal() => {
                eprintln!("{e}");
                pin = Some(ask_pin(&device.label())?);
            }
            other => break other?,
        }
    };
    match result {
        SendResult::Sent => println!("{}", t!("localsend.sent", name = device.label())),
        SendResult::Rejected => println!("{}", t!("localsend.rejected", name = device.label())),
    }
    Ok(())
}

/// Sends a text (or, without one, standard input) to the LocalSend app `to`.
async fn send_text(text: Option<String>, to: &str, pin: Option<&str>) -> Result<()> {
    // Read standard input first: a PIN can then only be typed when the text
    // was given as an argument.
    let text = match text {
        Some(text) => text,
        None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            buf.trim_end_matches(['\r', '\n']).to_string()
        }
    };
    let device = resolve(to).await?;
    println!("{}", t!("localsend.unverified", name = device.label()));
    let mut pin = pin.map(str::to_string);
    let result = loop {
        match client::send_text(&device, &text, &me(), pin.as_deref()).await {
            Err(e) if e.is::<client::PinRequired>() && std::io::stdin().is_terminal() => {
                eprintln!("{e}");
                pin = Some(ask_pin(&device.label())?);
            }
            other => break other?,
        }
    };
    match result {
        SendResult::Sent => println!("{}", t!("localsend.sent", name = device.label())),
        SendResult::Rejected => {
            println!("{}", t!("localsend.text_rejected", name = device.label()));
        }
    }
    Ok(())
}
