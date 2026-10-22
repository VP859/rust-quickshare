use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "rqs", author, version, about = "Bezpieczna aplikacja P2P do przesyłania plików", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Wysyła plik(i): do zaufanego urządzenia (`--to`) lub udostępnia przez jednorazowy TICKET.
    Send {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Alias, Node ID lub TICKET odbiorcy (musi mieć uruchomiony `rqs daemon` lub GUI).
        #[arg(short, long)]
        to: Option<String>,
    },
    /// Pobiera plik udostępniony przez TICKET.
    Receive {
        ticket: String,
        /// Katalog docelowy (domyślnie katalog Pobrane).
        output_dir: Option<PathBuf>,
    },
    /// Nasłuch w tle; zaufane urządzenia z Auto-Accept są przyjmowane automatycznie.
    Daemon {
        #[arg(short, long)]
        downloads_dir: Option<PathBuf>,
    },
    /// Interfejs terminalowy (ratatui).
    Tui,
    /// Wyświetla TICKET tego urządzenia (tekst + kod QR) do sparowania.
    Ticket,
    /// Zarządzanie zaufanymi urządzeniami.
    Peer {
        #[command(subcommand)]
        command: PeerCommands,
    },
    /// Diagnostyka sieci i konfiguracji.
    Doctor,
}

#[derive(Subcommand, Debug)]
pub enum PeerCommands {
    Add {
        /// TICKET lub Node ID urządzenia.
        ticket: String,
        #[arg(short, long)]
        alias: String,
        #[arg(long, default_value_t = false)]
        auto_accept: bool,
    },
    List,
    /// Zmienia alias lub ustawienie Auto-Accept.
    Edit {
        /// Alias lub Node ID.
        target: String,
        #[arg(short, long)]
        alias: Option<String>,
        #[arg(long)]
        auto_accept: Option<bool>,
    },
    Remove {
        target: String,
    },
}
