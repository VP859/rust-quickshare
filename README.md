# RQS P2P Quick Share (rust-quickshare)

**Secure, P2P, and VPN-friendly file sharing for Linux, Windows, macOS and Android.**

RQS transfers files directly between devices over QUIC. There is no storage server and no cloud;
devices find each other by **TICKET** (text or QR code) instead of mDNS or Bluetooth, so it keeps
working with a VPN enabled on either side.

## Features

* **End-to-end encryption** – QUIC + TLS 1.3 with a persistent Ed25519 identity per device (`NodeID`).
  Relays (fallback when hole punching fails) only forward encrypted packets.
* **VPN & NAT friendly** – [`iroh`](https://github.com/n0-computer/iroh): UDP hole punching,
  connection migration, encrypted relay fallback, NodeID lookup via DNS/pkarr.
* **Integrity** – BLAKE3 hash of every 1 MiB chunk and of the whole file.
* **Resume** – downloads go to a `.part` file; after a dropped connection (IP/VPN change) the
  transfer resumes from the last complete chunk.
* **Trusted devices** – alias + Auto-Accept per device; untrusted offers require confirmation.
* **Nearby devices + one-button pairing** – RQS apps on the same network show up
  automatically; **Sparuj** (Pair) adds a device without retyping a TICKET or scanning a QR code.
* **Interfaces** – desktop/Android GUI (Tauri v2), TUI (ratatui), CLI (clap), background daemon.

## Usage (CLI)

```bash
rqs discover                                 # devices on the local network
rqs pair "Laptop"                            # pairing: the other side confirms, both show the same code
rqs name "My laptop"                         # this device's name visible to others

rqs ticket                                   # this device's TICKET + QR code (pairing outside the LAN)
rqs peer add <TICKET> --alias "Laptop" --auto-accept   # + pairing request (--no-pair: only on this side)
rqs peer list | rqs peer edit Laptop --auto-accept false | rqs peer remove Laptop

rqs daemon                                   # receive in the background (Auto-Accept for trusted)
rqs send film.mkv notes.pdf --to Laptop      # send by alias, nearby name, NodeID or TICKET

rqs send film.mkv                            # share via a one-time TICKET (prints QR)
rqs receive <TICKET> ~/Downloads             # download a shared file

rqs tui                                      # terminal UI (see below)
rqs doctor                                   # network, local network discovery and config diagnostics
```

Set `RQS_CONFIG_DIR` to run several independent identities on one machine (e.g. for testing).
Configuration lives in `~/.config/rqs/` (`config.toml`, `secret.key`).

### TUI (`rqs tui`)

Panels: **[1] Trusted devices**, **[2] Nearby**, **[3] Transfers** (`Tab` / `1 2 3` switches, `↑↓` selects).

| Key | Action |
|-----|--------|
| `Enter` / `s` | send a file to the selected device (`Tab` completes the path; you can drag a file into the terminal) |
| `p` | pair with a nearby device |
| `i` / `+` | add a device by TICKET or Node ID (+ pairing request) |
| `e` / `a` / `x` | rename / toggle Auto-Accept / remove device |
| `o` / `f` | open the received file / its folder |
| `t` / `c` | this device's TICKET with QR code / copy TICKET |
| `g` / `N` | download folder / device name |
| `y` / `n` | answer a file offer or pairing request |
| `?` | help |

### Running as a service

| System  | File | Installation |
|---------|------|--------------|
| Linux   | `dist/systemd/rqs-daemon.service` | `systemctl --user enable --now rqs-daemon` (instructions in the file) |
| macOS   | `dist/launchd/com.rqs.daemon.plist` | `launchctl bootstrap gui/$(id -u) …` (instructions in the file) |
| Windows | `dist/windows/install-autostart.ps1` | Task Scheduler task at logon |

A daemon without a terminal accepts files **only** from devices with Auto-Accept.
The GUI has its own "Uruchamiaj przy starcie systemu" (launch at startup) toggle and keeps
running in the system tray.

## Discovery and pairing

* **No mDNS or Bluetooth.** Every node announces itself every 5 s with a small UDP packet (`RQSd`,
  port **47823**) to the broadcast addresses of its networks and to the multicast group
  `239.255.71.83` (TTL 1, also over loopback, so several instances on one machine can see each other).
  A new device gets an immediate unicast reply, so it's enough for the announcement to get
  through in one direction.
* **Pairing** (`Hello::Pair`): the other side asks the user for consent; both screens show the same
  6-digit code derived from both NodeIDs (BLAKE3). After acceptance both devices save each
  other as trusted, along with their current LAN address.
* The announcement only helps locate a device – identity is still authenticated by the NodeID in
  TLS. A daemon without a terminal rejects pairing.
* **When it doesn't work:** the firewall must let UDP 47823 through on the LAN; a VPN with a kill
  switch can block the local network (e.g. NordVPN: `nordvpn set lan-discovery on`). Then use
  **My TICKET** / QR – it works over the internet as well.
* Android holds a `MulticastLock` in the foreground service; its device name is the phone's model.
* When the list stays empty, every interface (GUI, TUI, `rqs discover`, `rqs doctor`) shows the
  reason: LAN sending blocked (VPN/firewall), no local network, or no announcements received.
* Adding a device manually (TICKET/Node ID) in any interface also sends a pairing request – once
  the other side accepts, both devices trust each other.
* A TICKET pasted with line breaks or spaces (wrapped in a terminal) is accepted; a truncated
  TICKET produces a clear error.

## Protocol (ALPN `rqs/0.2`)

```
dialer ─ bi stream ─▶ Hello::Push | Hello::Pull{token} | Hello::Pair{name, addr}
                     (Pair → PairResponse{accepted, name, addr}; otherwise one stream per file)
sender ────────────▶ TransferOffer {name, size, chunk_size=1MiB, blake3}
receiver ──────────▶ TransferDecision::Accept{start_chunk_index} | Reject{reason}
sender ─ uni stream per chunk (≤8 in flight) ─▶ ChunkHeader{index, len, blake3} + payload
receiver ──────────▶ TransferResult::Verified | Failed{reason}
```

* Frames: `postcard` with a 4-byte length prefix, max 64 KiB; offers with `chunk_size` 0 or
  > 16 MiB are rejected.
* File names are sanitized (path traversal, absolute paths, Windows reserved names); a received
  file never overwrites an existing one (`file (1).ext`).
* The `.part` name includes a hash fragment (`name.<hash>.part`), so a resume never appends
  data from a different file.

## Architecture

```
src/protocol/   messages and codec (no I/O)
src/node/       iroh endpoint, persistent identity, TICKET
src/transfer/   sender, receiver, security, service (shared accept loop + events)
src/discovery/  local network discovery (UDP broadcast + multicast, no mDNS)
src/pairing.rs  one-button pairing, verification code
src/config/     trusted devices, paths
src/cli/ src/tui/ src/daemon/   terminal interfaces
src-tauri/      GUI (desktop + Android), src-tauri/ui/index.html – frontend
src-tauri/gen/android/          Android project (Share Target, foreground service)
```

## Building

```bash
cargo build --release -p rqs_p2p            # CLI `rqs`
cargo test                                   # unit + end-to-end tests (two iroh nodes)

# GUI (Linux needs: libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev)
cd src-tauri && npx @tauri-apps/cli@2 dev
npx @tauri-apps/cli@2 build                  # .deb/.AppImage/.rpm | .msi/.exe | .dmg

# Android (ANDROID_HOME, NDK_HOME, JDK 17+)
cargo install tauri-cli --version "^2" --locked   # BuildTask.kt calls `cargo tauri`
cargo tauri android build --apk --aab
```

On Android: share any file from Gallery/Files → **RQS** → pick a recipient. The node stays
alive in the background thanks to a foreground service with a persistent notification. Received
files land in `Download/RQS`.

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`: CLI (cargo-dist), GUI installers
(Tauri) and APK/AAB are collected into a draft GitHub release with `SHA256SUMS.txt`.
Android signing: the `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`
and `ANDROID_KEY_PASSWORD` secrets.
