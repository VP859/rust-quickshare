# rust-quickshare

**Secure, P2P, and VPN-friendly file sharing for the modern web.**

`rust-quickshare` is a high-performance, decentralized file-sharing application built in Rust. Unlike traditional LAN-sharing tools that rely on mDNS or UDP Broadcast (which often break when a VPN is active), `rust-quickshare` uses **QUIC-based P2P** and **NAT Traversal** to ensure seamless transfers even through complex network configurations.

## 🚀 Key Features

*   **🔒 End-to-End Encryption:** All data is encrypted using QUIC (TLS 1.3) with Ed25519 identities. No server sees your files; the relay (DERP) only handles encrypted blobs.
*   **🌐 VPN & NAT Friendly:** Uses `iroh` for automatic UDP Hole Punching (STUN) and Connection Migration. If a direct P2P link isn't possible, it seamlessly falls back to an encrypted relay.
*   **⚡ Blazing Fast:** Built with `tokio` for async I/O and `blake3` for ultra-fast hashing. Supports resumable transfers and large files without blocking.
*   **🔍 No mDNS Required:** Discovers devices via Ticket/Pairing Codes (QR or text), bypassing traditional local network discovery limitations.
*   **🛠️ Resumable Transfers:** Automatically detects partial downloads and resumes from the last chunk, saving bandwidth and time.

## 🛠️ Tech Stack

*   **Core Language:** Rust (Edition 2021)
*   **P2P Network:** [`iroh`](https://github.com/n0-computer/iroh) (QUIC, STUN, DERP Relay)
*   **Async Runtime:** `tokio` (with `full` features)
*   **Hashing:** `blake3` (SIMD-optimized, multi-threaded)
*   **Serialization:** `bincode` (for fast metadata headers)
*   **Logging:** `tracing` + `tracing-subscriber`
*   **CLI/TUI:** `clap`, `ratatui`, `crossterm` (for terminal interface)
*   **GUI (Optional):** `tauri` or `egui`

## 🏗️ Architecture & Flow

`rust-quickshare` uses a ticket-based discovery and chunked streaming protocol.

1.  **Discovery:** The sender generates a **Ticket** (containing their Ed25519 Node ID). This can be shared via QR code or copied as text.
2.  **Connection:** The receiver uses the Ticket to initiate a **QUIC Handshake**. 
    *   *Direct Mode:* STUN servers help both devices punch holes through their NATs for a direct P2P UDP connection.
    *   *Relay Mode:* If NATs are symmetric or firewalls are strict, traffic is routed through an encrypted `iroh` relay (DERP).
3.  **Metadata Exchange:** The sender offers file details (name, size, BLAKE3 hash). The receiver accepts or rejects.
4.  **Streaming:** Files are split into chunks (e.g., 2MB). Each chunk is streamed with its index and hash for real-time verification.
5.  **Verification:** The receiver verifies each chunk on-the-fly. Upon completion, a final BLAKE3 hash is compared to ensure integrity.
6.  **Resume:** If the connection drops, the receiver checks for `.part` files and requests the transfer to resume from the last successful offset.

## 📦 Installation & Building

### Prerequisites
*   [Rust](https://www.rust-lang.org/tools/install) (latest stable)
*   Cargo

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/yourusername/rust-quickshare.git
cd rust-quickshare

# Build the project
cargo build --release

# Run the CLI
cargo run --bin quickshare -- --help
