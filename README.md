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
- **Pause and speed limit** – pause a transfer and resume it later (the connection stays open), and
  limit the speed of sending and receiving.
- **Several devices at once** – send the same files to several devices in one go.
- **History** – a searchable list of finished files and texts, kept only on this device (texts
  without their content); it can be cleared or turned off. The transfers list shows the current
  session and leads to it.
- **Every interface** – desktop and Android app (Tauri), terminal UI, command line and a
  background daemon; English and Polish.
- **Receive from iPhone and Mac** – an optional [LocalSend](https://localsend.org) compatibility
  mode lets the LocalSend app (iOS, macOS, Android, Windows, Linux) send files to Ferry
  ([below](#localsend-compatibility-mode)).

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
ferry peer verify Laptop             # safety number to compare with the other device
ferry peer list | ferry peer edit Laptop --auto-accept false | ferry peer remove Laptop
ferry send film.mkv                     # share one file through a one-time TICKET
ferry send photos/ --to Laptop --to Phone  # several devices at once
ferry history holiday                   # finished transfers, searched; --clear deletes them
ferry web photo.jpg                     # link + QR code for a browser: download it, send files back
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
| Auto-accept programs and scripts from Auto-Accept devices | `auto-accept-executables true` | `false` (they ask) |
| Copy texts from Auto-Accept devices without asking | `auto-accept-texts true` | `false` |
| Desktop app quits when its window is closed | `quit-on-close true` | `false` (keeps running in the tray) |
| Receive from LocalSend apps (iPhone, Mac) | `localsend true` | `false` |
| Speed limit (per second, sending and receiving separately) | `speed-limit 2M` | `off` |
| Keep a history of transfers | `history false` | `true` |

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
| Linux | `dist/systemd/ferry-daemon.service` (installed by the .deb/.rpm packages) | `systemctl --user enable --now ferry-daemon` – runs sandboxed; see the file for cargo installs |
| Windows | `dist/windows/install-autostart.ps1` | Task Scheduler task at logon |

A daemon without a terminal accepts files **only** from devices with Auto-Accept. The desktop app
has its own *Start at login* switch. Closing its window keeps it running in the system tray, or
quits it if you choose so in *Settings → Receiving*; starting Ferry again always brings back the
running window instead of opening a second one.

## Network modes

| | `local` (default) | `internet` | `relay` (own relay) |
|---|---|---|---|
| Servers contacted | none | public iroh relays and DNS run by n0, free of charge | only the relays you set |
| Devices reachable | on the same network (or VPN) | also on other networks (mobile data, another home) | also on other networks, through your relay |
| Finding paired devices | local network | local network, then n0 DNS by Node ID | local network, then the address saved at pairing |
| Encryption | end-to-end | end-to-end; relays forward only encrypted packets | end-to-end; your relay forwards only encrypted packets |
| Visible to servers | nothing | IP addresses, Node IDs and timing – never content | the same, but only to your relay |

Every mode tries a direct connection first; a relay is only the fallback when the devices cannot
reach each other directly. Change the mode with `ferry network local|internet`,
`ferry network relay https://relay.example.org`, `M` in the TUI or *Settings → Network*; it applies
after a restart. The desktop app always shows the active mode in the top bar.

### Own relay

The `relay` mode is the internet mode without n0. It needs an
[iroh relay](https://github.com/n0-computer/iroh/tree/main/iroh-relay) that both devices can reach,
for example on a small VPS or a home server with a forwarded port. Use the version of iroh-relay
that matches Ferry's iroh (1.4):

```bash
cargo install iroh-relay --version 1.4.1 --features server
iroh-relay --dev          
```

For use over the internet, run it with a configuration file that enables TLS (Let's Encrypt or your
own certificate) and give Ferry the `https://` address. Plain `http://` is accepted only for IP
addresses on this computer or the local network.

- Pair devices **after** both use the relay mode: without n0's lookup, a device is found at the
  address saved when pairing, which then contains the relay URL. Devices paired earlier are still
  found on the local network; pair them again to reach them through the relay.
- Both devices can also use different relays: Ferry reaches the other device through the relay
  named in its address.
- Anyone who knows the relay address can use the relay, unless you restrict it in its
  configuration. Your relay sees what n0's relays would: IP addresses, Node IDs, amounts and
  timing, never content.

## Local network and pairing

- Every device announces itself every 5 s on UDP port **47823** (broadcast and multicast
  `239.255.71.83`, no mDNS); transfers use UDP **47824** (files shared by TICKET use 47825–47834). Announcements are signed with the
  device key.
- **Firewall:** allow UDP 47823–47834 on the local network. With firewalld (Fedora, openSUSE)
  Ferry shows the command:
  `sudo firewall-cmd --permanent --add-port=47823-47834/udp && sudo firewall-cmd --reload`.
  VPNs with a kill switch may block the local network (NordVPN: `nordvpn set lan-discovery on`).
- **Android 17+** needs the *Nearby devices* permission for any local network traffic; Ferry asks
  at startup (Settings → Apps → Ferry → Permissions if it was denied).
- **Pairing** shows a six-digit code on both devices – pair only when they match. The code depends
  on random values from both devices, so an impostor cannot predict it.
- When the device list stays empty, every interface explains why (blocked sending, no network,
  nothing received).

## Browser sessions (no app on the other device)

A phone or computer without Ferry can exchange files with it through any web browser on the same
network: *Receive from a browser* (or *Open in a browser* after choosing files) in the GUI, or
`ferry web [files]`, shows a link and a QR code. The browser can download the shared files and
send files to Ferry; every batch from the browser is asked about before it is saved.

- The link contains a one-time token, works only for the first device that opens it, and stops
  after 30 minutes (`--minutes`) or when the session is stopped.
- It uses plain **HTTP** on TCP port **47835**: browsers do not accept the self-signed
  certificates a local server would need. Someone watching the local network could see the
  files, so use it only on networks you trust. With firewalld, Ferry shows the command that opens
  the port.

## LocalSend compatibility mode

Ferry has no app for iOS and macOS. With this mode on, iPhones, iPads and Macs – or any device
with the free [LocalSend](https://localsend.org) app – can **send files to Ferry**: Ferry appears in
LocalSend's list of nearby devices like another LocalSend device.

- Turn it on in *Settings → Privacy → Receive from LocalSend apps*, or
  `ferry config set localsend true` (the daemon and the TUI pick it up at their next start).
- It is **off by default** because LocalSend has a weaker security model than Ferry: there is no
  pairing and the sender is not verified – its name can be anything, and anyone on the local network
  can offer files. Ferry therefore **always asks** (never Auto-Accept, a daemon without a terminal
  declines), marks the offer as *LocalSend · not verified* with the sender's IP address, warns about
  programs and scripts, and makes a declined sender wait before it can offer again. The transfer is
  encrypted with TLS, but the sender is not authenticated.
- Only receiving is supported. Sending to LocalSend devices is not.
- It uses port **53317** (TCP for the transfers, UDP multicast `224.0.0.167` for discovery); allow
  both in the firewall: `sudo firewall-cmd --permanent --add-port=53317/tcp --add-port=53317/udp &&
  sudo firewall-cmd --reload`. The LocalSend app itself cannot run on the same computer at the same
  time, as both need the port.
- In hidden mode Ferry does not announce itself to LocalSend devices either; a LocalSend app
  can still reach it by IP address.

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
- [Testing](docs/TESTING.md) – tests, fuzzing, bad networks, benchmarks, Android by hand
- [Translating](docs/TRANSLATING.md) – improving or adding a language
- [Contributing](CONTRIBUTING.md) and the [changelog](CHANGELOG.md)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in Ferry by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
