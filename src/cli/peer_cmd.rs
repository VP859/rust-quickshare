use crate::config::Config;
use crate::node::NodeTicket;
use anyhow::{Context, Result};

pub fn handle_peer_cmd(cmd: crate::cli::args::PeerCommands) -> Result<()> {
    let mut config = Config::load()?;

    match cmd {
        crate::cli::args::PeerCommands::Add {
            ticket,
            alias,
            auto_accept,
        } => {
            let parsed_ticket = NodeTicket::deserialize_from_str(&ticket)
                .context("Nieprawidłowy bilet połączeniowy TICKET")?;
            let node_id = parsed_ticket.addr.id.to_string();

            config.add_peer(node_id.clone(), alias.clone(), auto_accept);
            config.save()?;

            println!(
                "[SUKCES] Dodano urządzenie '{}' ({}) (Auto-Accept: {}).",
                alias, node_id, auto_accept
            );
        }

        crate::cli::args::PeerCommands::List => {
            if config.peers.is_empty() {
                println!("Brak zapamiętanych urządzeń w konfiguracji (~/.config/rqs/config.toml).");
                return Ok(());
            }

            println!("\n=================== ZAUFANE URZĄDZENIA ===================");
            for (node_id, info) in &config.peers {
                println!(
                    " • Alias: {:<20} | Node ID: {} | Auto-Accept: {} | Dodano: {}",
                    info.alias,
                    node_id,
                    info.auto_accept,
                    info.added_at.format("%Y-%m-%d %H:%M")
                );
            }
            println!("==========================================================\n");
        }

        crate::cli::args::PeerCommands::Remove { target } => {
            if config.remove_peer(&target) {
                config.save()?;
                println!("[SUKCES] Usunięto urządzenie: '{}'", target);
            } else {
                println!("[BŁĄD] Nie znaleziono urządzenia dla wzorca: '{}'", target);
            }
        }
    }

    Ok(())
}
