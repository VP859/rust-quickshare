use crate::cli::args::PeerCommands;
use crate::config::Config;
use anyhow::{Context, Result};

pub fn handle_peer_cmd(cmd: PeerCommands) -> Result<()> {
    let mut config = Config::load()?;

    match cmd {
        PeerCommands::Add {
            ticket,
            alias,
            auto_accept,
        } => {
            let node_id = config
                .add_from_input(&ticket, &alias, auto_accept)
                .context("Nieprawidłowy bilet połączeniowy TICKET lub Node ID")?;
            config.save()?;

            println!(
                "[SUKCES] Dodano urządzenie '{}' ({}) (Auto-Accept: {}).",
                alias, node_id, auto_accept
            );
        }

        PeerCommands::List => {
            if config.peers.is_empty() {
                println!(
                    "Brak zapamiętanych urządzeń w konfiguracji ({}).",
                    Config::file_path()?.display()
                );
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

        PeerCommands::Edit {
            target,
            alias,
            auto_accept,
        } => {
            let key = config
                .find_key(&target)
                .with_context(|| format!("Nie znaleziono urządzenia: '{target}'"))?;
            if let Some(peer) = config.peers.get_mut(&key) {
                if let Some(alias) = alias {
                    peer.alias = alias;
                }
                if let Some(auto_accept) = auto_accept {
                    peer.auto_accept = auto_accept;
                }
                println!(
                    "[SUKCES] '{}' (Auto-Accept: {})",
                    peer.alias, peer.auto_accept
                );
            }
            config.save()?;
        }

        PeerCommands::Remove { target } => {
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
