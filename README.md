<p align="center"><img src="src-tauri/icons/icon.svg" width="96" alt="Ferry logo"></p>

<h1 align="center">Ferry</h1>

<p align="center"><b>Secure, peer-to-peer and VPN-friendly file sharing for Linux, Windows, macOS and Android.</b></p>

Ferry transfers files directly between devices over QUIC. There is no storage server and no cloud;
devices find each other by **TICKET** (text or QR code) instead of mDNS or Bluetooth, so it keeps
working with a VPN enabled on either side.

The command line tool is called `rqs`, and the crates keep their `rqs_p2p` / `rqs-gui` names.

## Features

* **End-to-end encryption** – QUIC + TLS 1.3 with a persistent Ed25519 identity per device (`NodeID`).
  Relays (fallback when hole punching fails) only forward encrypted packets.
* **VPN & NAT friendly** – [`iroh`](https://github.com/n0-computer/iroh): UDP hole punching,
  connection migration, encrypted relay fallback, NodeID lookup via DNS/pkarr.
* **Integrity** – BLAKE3 hash of every 1 MiB chunk and of the whole file.
* **Resume** – downloads go to a `.part` file; after a dropped connection (IP/VPN change) the
  transfer resumes from the last complete chunk.
* **Trusted devices** – alias + Auto-Accept per device; untrusted offers require confirmation.
  Several files sent at once are confirmed once for the whole batch (never more files or bytes
  than announced).
* **Nearby devices + one-button pairing** – Ferry apps on the same network show up
  automatically; **Pair** adds a device without retyping a TICKET or scanning a QR code.
* **Text and links** – send copied text or a link; after the receiver accepts it (or right away
  for Auto-Accept devices) it lands in their clipboard. On Android, links can be shared to Ferry
  from any app.
* **Interfaces** – desktop/Android GUI (Tauri v2), TUI (ratatui), CLI (clap), background daemon.
* **Languages** – English (default) and Polish in every interface (see [Language](#language)).

## Usage (CLI)

```bash
rqs discover                                 # devices on the local network
rqs pair "Laptop"                            # pairing: the other side confirms, both show the same code
rqs name "My laptop"                         # this device's name visible to others
rqs language pl                              # interface language (en, pl); no argument = show

rqs ticket                                   # this device's TICKET + QR code (pairing outside the LAN)
rqs peer add <TICKET> --alias "Laptop" --auto-accept   # + pairing request (--no-pair: only on this side)
rqs peer list | rqs peer edit Laptop --auto-accept false | rqs peer remove Laptop

rqs daemon                                   # receive in the background (Auto-Accept for trusted)
rqs send film.mkv notes.pdf --to Laptop      # send by alias, nearby name, NodeID or TICKET

rqs text "https://example.org" --to Laptop # text/link → the receiver's clipboard (stdin when omitted)

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
| `T` | send text or a link to the selected device |
| `p` | pair with a nearby device |
| `i` / `+` | add a device by TICKET or Node ID (+ pairing request) |
| `e` / `a` / `x` | rename / toggle Auto-Accept / remove device |
| `o` / `f` | open the received file / its folder |
| `t` / `c` | this device's TICKET with QR code / copy TICKET |
| `g` / `N` | download folder / device name |
| `L` | switch the interface language |
| `y` / `n` | answer a file offer or pairing request |
| `?` | help |

### Running as a service

| System  | File | Installation |
|---------|------|--------------|
| Linux   | `dist/systemd/rqs-daemon.service` | `systemctl --user enable --now rqs-daemon` (instructions in the file) |
| macOS   | `dist/launchd/com.rqs.daemon.plist` | `launchctl bootstrap gui/$(id -u) …` (instructions in the file) |
| Windows | `dist/windows/install-autostart.ps1` | Task Scheduler task at logon |

A daemon without a terminal accepts files **only** from devices with Auto-Accept.
The GUI has its own "Start at login" toggle and keeps
running in the system tray.

## Language

English is the default. The language is chosen in this order:

1. `--lang <code>` on the command line (`rqs --lang pl tui`),
2. the `RQS_LANG` environment variable,
3. `language` in `config.toml`, saved by `rqs language <code>`, the `L` key in the TUI or
   **Settings → Language** in the GUI (the tray menu follows it too),
4. English.

The Android parts outside the web view (notification, foreground service) use the phone's system
or per-app language (Settings → Apps → Ferry → Language). Log messages are always
in English. Translations live in `locales/app.yml` (backend) and in `src-tauri/ui/index.html`
(GUI); a unit test checks that every key has both languages.

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
* **When it doesn't work:** the firewall must let UDP 47823 (discovery) and 47824 (QUIC transfers)
  through on the LAN. firewalld (default on Fedora and openSUSE) is detected and the app shows the
  exact command: `sudo firewall-cmd --permanent --add-port=47823-47824/udp && sudo firewall-cmd --reload`.
  A VPN with a kill switch can block the local network (e.g. NordVPN: `nordvpn set lan-discovery on`).
  Then use **My TICKET** / QR – it works over the internet as well.
* Android holds a `MulticastLock` in the foreground service; its device name is the phone's model.
* **Android 17+** (targetSdk 37) blocks all local network traffic (broadcast, multicast, direct
  LAN connections) without the **ACCESS_LOCAL_NETWORK** permission ("Nearby devices" group).
  The app requests it at startup; if it was denied: Settings → Apps → Ferry →
  Permissions → Nearby devices. Without it only transfers through the internet relay work.
* Android logs (Rust + Kotlin): `adb logcat -s RqsCore RqsService RqsShare`.
* When the list stays empty, every interface (GUI, TUI, `rqs discover`, `rqs doctor`) shows the
  reason: LAN sending blocked (VPN/firewall), no local network, or no announcements received.
* Adding a device manually (TICKET/Node ID) in any interface also sends a pairing request – once
  the other side accepts, both devices trust each other.
* A TICKET pasted with line breaks or spaces (wrapped in a terminal) is accepted; a truncated
  TICKET produces a clear error.

## Protocol (ALPN `rqs/0.2`)

```
dialer ─ bi stream ─▶ Hello::Push | Hello::Pull{token} | Hello::Pair{name, addr} | Hello::Text{text}
                     | Hello::Batch{count, total_size, names}
                     (Pair → PairResponse{accepted, name, addr}; Text → TextDecision;
                      Batch → BatchDecision; otherwise one stream per file)
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
src/config/     trusted devices, paths, saved language
src/i18n.rs     interface language selection (locales/app.yml)
src/cli/ src/tui/ src/daemon/   terminal interfaces
src-tauri/      GUI (desktop + Android), src-tauri/ui/index.html – frontend
src-tauri/gen/android/          Android project (Share Target, foreground service)
```

## Building

```bash
cargo build --release -p rqs_p2p            # CLI `rqs`
cargo test                                   # unit + end-to-end tests (two iroh nodes)
cargo bench --bench transfer_benchmarks      # hashing, framing and a 64 MiB transfer over QUIC
cargo doc --no-deps -p rqs_p2p -p rqs-gui --open   # API documentation

# GUI (Linux needs: libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev)
cd src-tauri && npx @tauri-apps/cli@2 dev
npx @tauri-apps/cli@2 build                  # .deb/.AppImage/.rpm | .msi/.exe | .dmg

# Android (ANDROID_HOME, NDK_HOME, JDK 17+)
cargo install tauri-cli --version "^2" --locked   # BuildTask.kt calls `cargo tauri`
cargo tauri android build --apk --aab
```

On Android: share any file from Gallery/Files → **Ferry** → pick a recipient. The node stays
alive in the background thanks to a foreground service with a persistent notification. Received
files land in `Download/Ferry`; photos and videos are moved to `Pictures/Ferry`, so they show up
in the gallery as a "Ferry" album. Sharing a link or text to Ferry opens the "Send text" dialog.

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`: CLI (cargo-dist), GUI installers
(Tauri) and APK/AAB are collected into a draft GitHub release with `SHA256SUMS.txt`.
Android signing: the `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`
and `ANDROID_KEY_PASSWORD` secrets.
