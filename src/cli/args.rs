use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "rqs", author, version, about = "Secure P2P file transfer application", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Send,
    Receive {
        ticket: String,
    },
}