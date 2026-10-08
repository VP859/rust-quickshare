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
    Send {
        file_path: PathBuf,
    },
    Receive {
        ticket: String,
        output_path: Option<PathBuf>,
    },
    Daemon {
        #[arg(short, long)]
        downloads_dir: Option<PathBuf>,
    },
    Tui,
    Peer {
        #[command(subcommand)]
        command: PeerCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum PeerCommands {
    Add {
        ticket: String,
        #[arg(short, long)]
        alias: String,
        #[arg(long, default_value_t = false)]
        auto_accept: bool,
    },
    List,
    Remove {
        target: String,
    },
}
