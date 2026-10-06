use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info};

use rust_quickshare::cli::{Cli, Commands};
use rust_quickshare::node::{NodeEngine, NodeTicket, load_or_generate_secret_key};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum PingPongMsg {
    Ping,
    Pong,
}

async fn write_frame<W: AsyncWriteExt + Unpin>(mut writer: W, msg: &PingPongMsg) -> Result<()> {
    let bytes = postcard::to_allocvec(msg).context("Message encoding error in Postcard")?;
    let len = bytes.len() as u32;
    writer.write_u32(len).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncReadExt + Unpin>(mut reader: R) -> Result<PingPongMsg> {
    let len = reader.read_u32().await.context("Frame length read error")?;
    let mut buf = vec![0u8; len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .context("Error reading frame content")?;
    let msg = postcard::from_bytes(&buf).context("Error decoding Postcard message")?;
    Ok(msg)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let secret_key = load_or_generate_secret_key(None)?;
    let engine = NodeEngine::new(secret_key).await?;

    match cli.command {
        Commands::Send => {
            let addr = engine.node_addr().await?;
            let ticket = NodeTicket::new(addr);
            let ticket_str = ticket.serialize_to_string()?;

            println!("\n==================================================================");
            println!(" The sender node is ready.");
            println!(" Share the TICKET below with the recipient:");
            println!("\n {}", ticket_str);
            println!("==================================================================\n");

            info!("Waiting for an incoming call...");
            let conn = engine.accept().await?;

            let (mut send, mut recv) = conn
                .accept_bi()
                .await
                .context("Error opening bidirectional stream")?;
            debug!("A bidirectional stream has been opened.");

            let msg = read_frame(&mut recv).await?;
            info!("Message received: {:?}", msg);

            if msg == PingPongMsg::Ping {
                write_frame(&mut send, &PingPongMsg::Pong).await?;
                info!("Response sent: PONG");
                println!("\n[SUCCESS] Ping-Pong protocol completed successfully!");
            } else {
                anyhow::bail!("An unexpected message was received: {:?}", msg);
            }
        }

        Commands::Receive { ticket } => {
            let parsed_ticket = NodeTicket::deserialize_from_str(&ticket)
                .context("Invalid connecting ticket format")?;

            info!(
                "Connecting to the target node: {}",
                parsed_ticket.addr.node_id
            );
            let conn = engine.connect(parsed_ticket.addr).await?;

            let (mut send, mut recv) = conn
                .open_bi()
                .await
                .context("Failed to open the bidirectional stream.")?;
            debug!("A bidirectional stream has been opened.");

            let start_time = Instant::now();

            info!("Sending PING frame...");
            write_frame(&mut send, &PingPongMsg::Ping).await?;

            let response = read_frame(&mut recv).await?;
            let rtt = start_time.elapsed();

            if response == PingPongMsg::Pong {
                println!("\n[SUCCESS] PONG response received!");
                println!("RTT (Round Trip Time):{:.2?}", rtt);
            } else {
                anyhow::bail!("Received an incorrect response: {:?}", response);
            }
        }
    }

    Ok(())
}
