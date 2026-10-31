# Changelog

All notable changes to Ferry are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/) (while in `0.x`, a minor version may break
compatibility).

## [Unreleased]

### Breaking

- The project is now called **Ferry** everywhere: crates `ferry` and `ferry-gui`, ALPN `ferry/1`,
  TICKET prefix `ferry:`, discovery magic `FRYd`, Android package `io.github.vp859.ferry`,
  environment variables `FERRY_CONFIG_DIR` and `FERRY_LANG`. Devices running older versions cannot
  talk to this one. An existing `~/.config/rqs` folder is moved to `~/.config/ferry` on first start.
- Pairing uses a new exchange (commit-reveal nonces), so pairing codes differ from older versions.
- Discovery announcements are signed; unsigned announcements of older versions are ignored.
- macOS and iOS are no longer supported.

### Added

- **Transfers survive broken connections**: when the connection breaks (a phone falling asleep,
  Wi-Fi dropping), the sender reconnects up to three times and the receiver resumes the partial
  file without asking the user again. Interfaces show "Connection lost – reconnecting (n/3)".
- **Faster transfers over lossy links** in internet mode: BBRv3 congestion control instead of
  Cubic – 7× faster at 1 % packet loss and about 20× at 3 % in tests. Local mode keeps Cubic,
  which is faster on clean, fast links.
- **Android background**: the foreground service is of type `connectedDevice`, which Android 15+
  does not stop after 6 hours (`dataSync` remains a fallback and stops cleanly with a notification
  when its time is up); wake and Wi-Fi locks while transfers run; *Settings → Background* shows
  battery optimization, battery saver, background restriction and data saver and opens the system
  app settings.
- Tests: property-based tests (proptest), fuzz targets for every parser (`fuzz/`), network tests
  with packet loss, a network change and outages, `tc netem` in CI, a coverage threshold (78 %)
  and benchmark comparisons against the base branch on pull requests. See `docs/TESTING.md`.

- **Folders**: send whole folders with their structure (CLI `ferry send <folder>`, *Choose a
  folder* in the desktop GUI, paths typed in the TUI).
- **Cancel transfers**: the ✕ button in the GUI, `x` in the TUI transfers panel. A partly
  received file is resumed by the next transfer of the same file.
- **Hidden mode**: do not announce this device on the local network; only paired devices see it
  (*Settings → Privacy*, `H` in the TUI, `ferry config set hidden true`).
- `auto_accept_texts` setting: texts from Auto-Accept devices are copied to the clipboard without
  asking only when it is enabled (off by default).
- `ferry config list|get|set|unset|path` for every setting.
- Desktop: choose what closing the window does – keep running in the background (tray, default)
  or quit Ferry (*Settings → Receiving*, `ferry config set quit-on-close true`). The first time
  the window is hidden, a notification says that Ferry keeps running.
- Every transfer shows whether it goes directly or through a relay.
- **Chunk size follows the file size**: 1 MiB up to 1 GiB, growing to 8 MiB for files above
  4 GiB (about 4% faster on a 3 GiB file and far fewer streams), with about 8 MiB in flight.
- **Large files**: the receiver checks free disk space (keeping 256 MiB) before accepting a file or
  a batch and declines with a reason instead of filling the disk.
- `ferry::error::kind` classifies library errors (cancelled, connection, timeout, protocol,
  integrity, unsafe path, disk full, …) through typed errors.
- `ferry doctor` exits with code 1 when a check fails.
- `ferry identity show|export|import` to back up and restore the device key.
- GUI: the top bar shows the active network mode and whether the device is hidden; *Settings →
  Privacy* explains what leaves the device; keyboard focus stays inside dialogs and returns to
  where it was; finished transfers are announced to screen readers.
- Documentation: security model, protocol, architecture, building and translating guides;
  `SECURITY.md`, `CONTRIBUTING.md`, Code of Conduct, issue and pull request templates.
- Dual license MIT OR Apache-2.0.

### Fixed

- File names such as `". A"` were sanitized to `" A"` (leading space), and sanitizing again gave
  another name; dots and whitespace are now trimmed together (found by the property tests).
- Errors of a broken QUIC connection read through `tokio::io` were classified as local I/O errors.

- GUI: Tab could leave an open dialog (icons were taken for focusable elements), and the controls
  of closed dialogs stayed reachable with Tab and screen readers.
- Desktop: starting Ferry again (e.g. from the app menu) shows the running window instead of
  opening another instance.
- Transfer progress shows what actually arrived: the receiver reports its progress to the sender
  a few times per second, so the sender no longer jumps to 100% while data is still on the way,
  and the receiver no longer stays at 0% until a whole 1 MiB chunk has arrived.

### Security

- Pairing codes depend on random nonces exchanged with a commit-reveal scheme; previously the code
  depended only on the two Node IDs and could be matched by an attacker generating keys.
- Discovery announcements are signed with the device key and expire after 5 minutes, so nobody
  can announce false addresses for someone else's device.
- Rate limits for questions to the user: at most 2 open requests per device and 8 in total, and a
  30-second pause after a declined pairing request.
- Received folders are resolved through the file system and must stay inside the download folder
  (also with symbolic links); the sender does not follow links.
- Android: app data (including the device key) is excluded from cloud backups and device
  transfers; unused storage permissions were removed.

### Changed

- The workspace has three crates: the `ferry` library (no terminal or GUI dependencies), `ferry-cli`
  (`cli/`, the `ferry` binary) and `ferry-gui`. CLI release archives are named `ferry-cli-…`.
- The GUI frontend is split into `index.html`, `styles.css` and ES modules with one translation
  file per language; the Content-Security-Policy no longer allows inline scripts. CI lints it with
  Biome and runs Playwright tests against a mocked Tauri API.
- The terminal UI's state module is split by topic; `rustfmt.toml` documents the formatting.
- Senders no longer wait for each chunk's acknowledgement; peers must not send chunks smaller than
  16 KiB.

- CI checks formatting without changing files, runs on Linux and Windows, checks the minimum Rust
  version (1.91), the Android build, licenses and advisories (`cargo-deny`) and the GUI frontend.
  Release builds publish an SBOM and build provenance attestations; all actions are pinned to
  commit SHAs and updated by Dependabot.
- Release artifacts are built for Linux, Windows and Android only.

Earlier versions were published privately as "RQS P2P Quick Share" without a changelog.
