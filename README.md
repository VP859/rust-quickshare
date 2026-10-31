<p align="center"><img src="src-tauri/icons/icon.svg" width="96" alt="Ferry logo"></p>

<h1 align="center">Ferry</h1>

<p align="center"><b>Private, peer-to-peer file sharing for Linux, Windows and Android.</b></p>

<p align="center">
  <a href="LICENSE-MIT"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue"></a>
  <img alt="Rust 1.91+" src="https://img.shields.io/badge/rust-1.91%2B-orange">
</p>

Ferry sends files, folders and text directly from one device to another over QUIC, end-to-end
encrypted. There is no account, no cloud and no server in between. Devices on the same network
find each other automatically; others connect with a **TICKET** (text or QR code). Optionally,
paired devices can also reach each other from different networks through free public relays – see
[Network modes](#network-modes).

**No telemetry.** Ferry has no analytics, no crash reporting, no update checks and never contacts
its authors. In the default mode it does not contact any server at all.

## Features

- **End-to-end encryption** – QUIC + TLS 1.3 with a persistent Ed25519 key per device (the Node ID).
- **Integrity and resume** – every chunk and the whole file are checked with BLAKE3;
  interrupted transfers resume where they stopped.
- **Files, folders and text** – send several files or whole folders (their structure is kept), or
  a link that lands in the other device's clipboard after confirmation.
- **Nearby devices and one-button pairing** – Ferry devices on the same network appear on their
  own; pairing shows the same six-digit code on both screens.
- **Trusted devices** – an alias and optional Auto-Accept per device; everything else asks first.
- **Privacy controls** – hide the device from local network discovery except for paired devices,
  and see exactly what leaves the device (*Settings → Privacy*, [details](docs/SECURITY-MODEL.md#what-each-party-can-see)).
- **Cancel any transfer** – a partly received file is resumed next time.
- **Every interface** – desktop and Android app (Tauri), terminal UI, command line and a
  background daemon; English and Polish.

## Install

Ferry is not published yet. Build it from source as described in
[docs/BUILDING.md](docs/BUILDING.md):

```bash
cargo install --path cli --locked   # the `ferry` command line tool
cd src-tauri && npx @tauri-apps/cli@2 build   # desktop app (.deb/.rpm/.AppImage, .msi/.exe)
```

## Quick start

```bash
ferry discover                          # devices on the local network
ferry pair "Laptop"                     # the other device confirms; both show the same code
ferry send photo.jpg Holiday/ --to Laptop   # files and folders
ferry text "https://example.org" --to Laptop
ferry daemon                            # receive in the background
ferry tui                               # terminal user interface
```

More commands:

```bash
ferry ticket                            # this device's TICKET and QR code
ferry peer add <TICKET> --alias Laptop --auto-accept
ferry peer list | ferry peer edit Laptop --auto-accept false | ferry peer remove Laptop
ferry send film.mkv                     # share one file through a one-time TICKET
ferry receive <TICKET> ~/Downloads      # download a shared file
ferry config                            # all settings (also: get, set, unset, path)
ferry config set hidden true            # hide this device from unpaired devices
ferry identity export ~/ferry-key.txt   # back up the device key (keep it secret)
ferry doctor                            # network and discovery diagnostics
```

### Settings

| Setting | `ferry config set …` | Default |
|---------|----------------------|---------|
| Device name | `device-name "My laptop"` | host name |
| Language | `language pl` | English |
| Network mode | `network-mode internet` | `local` |
| Download folder | `download-dir ~/Inbox` | system Downloads folder |
| Hidden from unpaired devices | `hidden true` | `false` |
| Copy texts from Auto-Accept devices without asking | `auto-accept-texts true` | `false` |
| Desktop app quits when its window is closed | `quit-on-close true` | `false` (keeps running in the tray) |

Settings are stored in `~/.config/ferry/config.toml` (Linux), `%APPDATA%\ferry\config.toml`
(Windows) or the app's private storage (Android). Environment variables:

- `FERRY_CONFIG_DIR` – use another configuration folder (e.g. two devices on one computer),
- `FERRY_LANG` – interface language for this run (`en`, `pl`),
- `RUST_LOG` – log detail (`ferry=debug`).

### Terminal UI (`ferry tui`)

Panels: **[1] Trusted**, **[2] Nearby**, **[3] Transfers** (`Tab` or `1 2 3` to switch, `↑↓` to select).

| Key | Action |
|-----|--------|
| `Enter` / `s` | send a file or folder to the selected device (`Tab` completes paths; drag files into the terminal) |
| `T` | send text or a link |
| `p` | pair with a nearby device |
| `i` / `+` | add a device by TICKET or Node ID |
| `e` / `a` / `x` | rename / toggle Auto-Accept / remove a device |
| `x` (transfers panel) | cancel the selected transfer |
| `o` / `f` / `C` | open the received file / its folder / clear finished transfers |
| `t` / `c` | show / copy this device's TICKET |
| `g` / `N` / `L` | download folder / device name / language |
| `M` / `H` | network mode / hidden mode |
| `y` / `n` | answer an offer or pairing request |
| `?` | help |

### Running in the background

| System | File | Setup |
|--------|------|-------|
| Linux | `dist/systemd/ferry-daemon.service` | `systemctl --user enable --now ferry-daemon` (see the file) |
| Windows | `dist/windows/install-autostart.ps1` | Task Scheduler task at logon |

A daemon without a terminal accepts files **only** from devices with Auto-Accept. The desktop app
has its own *Start at login* switch. Closing its window keeps it running in the system tray, or
quits it if you choose so in *Settings → Receiving*; starting Ferry again always brings back the
running window instead of opening a second one.

## Network modes

| | `local` (default) | `internet` |
|---|---|---|
| Servers contacted | none | public iroh relays and DNS run by n0, free of charge |
| Devices reachable | on the same network (or VPN) | also on other networks (mobile data, another home) |
| Encryption | end-to-end | end-to-end; relays forward only encrypted packets |
| Visible to servers | nothing | IP addresses, Node IDs and timing – never content |

Change it with `ferry config set network-mode internet`, `M` in the TUI or *Settings → Network*;
it applies after a restart. The desktop app always shows the active mode in the top bar.

## Local network and pairing

- Every device announces itself every 5 s on UDP port **47823** (broadcast and multicast
  `239.255.71.83`, no mDNS); transfers use UDP **47824**. Announcements are signed with the
  device key.
- **Firewall:** allow UDP 47823–47824 on the local network. With firewalld (Fedora, openSUSE)
  Ferry shows the command:
  `sudo firewall-cmd --permanent --add-port=47823-47824/udp && sudo firewall-cmd --reload`.
  VPNs with a kill switch may block the local network (NordVPN: `nordvpn set lan-discovery on`).
- **Android 17+** needs the *Nearby devices* permission for any local network traffic; Ferry asks
  at startup (Settings → Apps → Ferry → Permissions if it was denied).
- **Pairing** shows a six-digit code on both devices – pair only when they match. The code depends
  on random values from both devices, so an impostor cannot predict it.
- When the device list stays empty, every interface explains why (blocked sending, no network,
  nothing received).

## Android

Share any file or link from another app to **Ferry** and pick a recipient. A foreground service
with a notification keeps Ferry reachable in the background. Received files are saved to
`Download/Ferry`; photos and videos go to `Pictures/Ferry` so they appear in the gallery.
Ferry's data is excluded from Android backups – a new phone pairs again.

## Security

Read [docs/SECURITY-MODEL.md](docs/SECURITY-MODEL.md) for what Ferry protects against and its known
limits, and [SECURITY.md](SECURITY.md) to report a vulnerability privately. Ferry has not been
audited.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) – how the crates and interfaces fit together
- [Protocol](docs/PROTOCOL.md) – wire format, pairing and discovery
- [Building](docs/BUILDING.md) – desktop, Android, checks and releases
- [Translating](docs/TRANSLATING.md) – improving or adding a language
- [Contributing](CONTRIBUTING.md) and the [changelog](CHANGELOG.md)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in Ferry by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
