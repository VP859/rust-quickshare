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
}
