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

- **Android: "Open" for received files**, as on the desktop: the file opens in the app chosen for
  its type, which gets read access to this one file only (a FileProvider `content://` URI).
- **Auditable release binaries**: `ferry` (archives, `.deb`, `.rpm`) and `ferry-gui` (all desktop
  packages) are built with `cargo auditable`, so `cargo audit bin`, Syft or Trivy can list their
  dependencies and check them for known vulnerabilities; the release fails when a binary lacks the
  list. **OpenSSF Scorecard** checks of the project run weekly in CI.
- **Shell completions and manual pages** for the `ferry` CLI: `ferry completions bash|zsh|fish|powershell|elvish`,
  and in the `.deb`/`.rpm` packages and the release archives ready-made completions and a manual
  page per command (`man ferry`, `man ferry-send`, …), generated from the command line itself.
- **Relay check**: *Check* next to the relay addresses in the settings and `ferry doctor` connect to
  each relay for real and name what fails (unknown name, closed port, a certificate that is not
  accepted, a server that is not an iroh relay) or show the round trip. `ferry doctor` fails when
  none of the relays of a relay mode works.
- A transfer that receives no data for 3 s shows *no data for 0:05* instead of its last speed.
- **Speed and time left** for running transfers in the GUI (`45% · 120 MB / 260 MB · 11.4 MB/s ·
  0:12 left`), smoothed over a few seconds and shown after the first second, so it does not jump.
- **Relay-only mode** (`relay-only`, *Settings → Network → Only through own relay*): every
  connection goes through your own relays, never directly, even in the same network. Ferry opens
  no UDP socket for direct paths, so the other device sees only the relay's address instead of your
  IP and the network in between cannot tell which two devices talk. Slower than a direct path.
  `ferry network relay-only URL`, `M` in the TUI cycles through it.
- **Android: permissions with a reason.** Android's dialog appears on the first start, and again for
  *Local network* (Android 17+) when a search starts; the camera is asked for only when a QR code
  is scanned (before, the scanner failed until the camera was allowed in the settings). A card says
  why Ferry needs whatever is still missing and opens the app settings once Android no longer
  asks; "Not now" leaves a hint under *Nearby*.
- **"Why?" links** next to the existing hints (devices that do not show up, the pairing code,
  LocalSend, the network mode, a phone that cannot open a browser link) open the matching topic of
  the help window; `npm run check:ui` checks that every one names a topic.
- **`ferry guide [TOPIC]`**: the same topics on the command line, with the commands to use; the TUI
  tells new users (nothing paired yet) how to pair and where the help is.
- **Help in the app**: a **?** button in the GUI (desktop and Android) with short answers – sending,
  pairing and the code, devices without Ferry, iPhone and Mac, local or internet, devices that do
  not show up, what others can see – each linked to its section of the documentation (checked by
  `npm run check:ui`). New users see a small welcome card with three steps, never a dialog; it goes
  away for good when closed or after the first pairing or transfer.
- **Encrypted browser sessions**: the files, their names and the list of shared files go
  end-to-end encrypted (ChaCha20-Poly1305 in 64 KiB records) with a key that is only in the link
  after `#` and never crosses the network; the page decrypts and encrypts with `@noble/ciphers`.
  Someone listening on the network sees only sizes and timing. Over plain HTTP it cannot protect
  against someone who changes the page on the way, which the interfaces say.
- **Remove location from photos** (`ferry config set strip-metadata true`, *Settings → Privacy*,
  off by default): JPEG and PNG photos sent to Ferry devices and LocalSend apps go without their
  EXIF data (GPS position, time, camera, serial number – only the orientation is kept), XMP, IPTC,
  comments and appended data. The image data is copied as it is, so the picture is identical; a
  damaged photo is not sent rather than sent with its metadata.
- **Cancel and delete**: a running download from a Ferry device can be cancelled together with
  what was received so far (the bin button in the GUI, `X` in the TUI), instead of keeping the
  `.part` file for resuming.
- **Send again from the history**: a file that was sent (or received) and is still on this device
  goes out again to the same Ferry device or to a recipient chosen now (GUI), or with
  `ferry history --send-again N`; the history keeps the path of sent files for this.
- **Send the clipboard** from the tray menu: the text opens ready to be sent to a chosen device.
- **Accessibility tests** (`tests/gui/accessibility.spec.js`): axe-core (WCAG 2.0–2.2 A and AA) and
  an own audit of names, labels, dialogs and contrast of the GUI and the browser session page in
  both themes, and the keyboard. axe also found icon-only buttons without a name on phones and a
  device row that was a button holding other buttons; both are fixed. Their first run
  found too little contrast for secondary text, warnings and the dark theme's buttons; the colours
  were adjusted. The history tabs follow the ARIA tab pattern (arrow keys), animations stop with
  *reduce motion*, and the frontend check refuses a translation key written twice.
- **Sending to LocalSend apps** (iPhone, iPad, Mac, and every other LocalSend device): files and
  folders, with the recipient list's *LocalSend apps* section in the GUI and `ferry localsend scan`
  / `ferry localsend send FILE… --to NAME|IP [--pin PIN]` on the command line. LocalSend apps also
  appear under *Nearby* in the GUI and *Search* finds them; a PIN is asked for when an app requires
  one; Ferry presents its certificate to apps that require the sender's (mutual TLS). The server
  certificate must be the announced one; the receiver's name is not verified, which the interfaces
  say before sending.
- **Sealed discovery answers**: a hidden device now answers its paired devices in a packet
  encrypted with a secret made at pairing, so nothing in it (name, Node ID, operating system,
  marker) is readable on the network. Devices paired earlier are answered in the clear until they
  are paired again. `config.toml` is now readable only by its owner, since it holds these secrets.
- **Operating system not announced** unless `ferry config set announce-platform true` (*Settings →
  Privacy*): on other devices it only chooses an icon.
- **Random port** (`ferry config set random-port true`, *Settings → Privacy*): direct connections
  no longer listen on the fixed UDP port 47824.
- **Only paired devices** (`ferry config set paired-only true`, *Settings → Privacy*): offers,
  batches, texts and pairing requests from unknown devices are declined without asking, so that
  knowing a Node ID or having an old TICKET is no longer enough to ask. A new device is paired
  from the device that has the setting, or after `ferry pairing` / *Open pairing* has opened
  pairing for 5 minutes.
- **Device key in the system keyring** (optional, desktop): `ferry identity storage system` or
  *Settings → Privacy* moves the key into the Secret Service (KWallet, GNOME Keyring) or the Windows
  Credential Manager, and `ferry identity storage file` back; the Node ID and pairings stay the
  same. The move is verified before the key file is overwritten and removed.
- **"Send with Ferry" in file managers**: the context menu of Dolphin, Nautilus (with
  nautilus-python) and Nemo, installed by the deb and rpm packages or for one user with
  `ferry integrate`, and *Send to → Ferry* in Windows File Explorer (NSIS installer). The desktop
  app takes files and folders on its command line (`ferry-gui -- PATH…`); when it already runs, the
  new start hands them over and the running app asks for the recipient.
- **Pause and speed limit**: pause a running transfer and resume it (GUI button, `p` in the TUI);
  the connection stays open. A speed limit for sending and, separately, receiving
  (`ferry config set speed-limit 2M`, *Settings*) applies to all transfers, also from LocalSend
  and browsers.
- **Send to several devices at once**: `ferry send … --to A --to B`, or tick several devices in the
  GUI. Every file is hashed once for all of them.
- **History of transfers**: finished transfers are kept on this device (`history.jsonl`, the last
  1000) – `ferry history [search] [--kind files|texts] [--clear]`, *History* in the GUI with search,
  *Files* / *Texts* tabs and *Clear history*. Texts are recorded without their content. The empty
  transfers list links to it.
  Turn it off with `ferry config set history false` or in *Settings → Privacy*.
- **Own relay mode** (`relay`): the internet mode without n0. Connections go directly when
  possible, otherwise only through the relays you set (`ferry network relay https://…`,
  `ferry config set relay-urls`, *Settings → Network*), and nothing is published to or looked up
  from n0's DNS. Paired devices are reached at the address saved at pairing. Tested against a real
  iroh relay server (`tests/relay.rs`). The README explains how to run one.
- **Browser sessions** (like PairDrop, without a server): `ferry web [files]` or *Receive from a
  browser* / *Open in a browser* in the GUI shows a link and QR code. Any phone or computer on the
  same network can download the shared files and send files to Ferry in its web browser, with no
  app. The link has a one-time token, works only for the first device that opens it, expires after
  30 minutes, and every batch from the browser is asked about. Plain HTTP on TCP 47835, so it is
  meant for trusted networks (`docs/SECURITY-MODEL.md`).
- **LocalSend compatibility mode** (off by default): iPhones, Macs and every other device with the
  LocalSend app can send files and folders to Ferry, which appears in LocalSend's device list.
  Senders are not verified (LocalSend has no pairing), so every offer is asked about, labelled
  *LocalSend · not verified* with the sender's address, and never accepted automatically. Uses
  TCP/UDP port 53317. *Settings → Privacy*, `ferry config set localsend true`. Tested against a
  client written from the LocalSend specification and an independent Python client
  (`scripts/localsend-client.py`).
- **Warning about files that can run code**: programs and installers (`.exe`, `.msi`, `.apk`,
  `.AppImage`, …), scripts and shortcuts (`.bat`, `.ps1`, `.sh`, `.desktop`, `.lnk`, …), Office files
  with macros and names that hide their extension with direction-changing characters are marked
  in red in the GUI and TUI before you accept. Such files are never accepted automatically – not
  even from Auto-Accept devices and not as part of an accepted batch – unless you turn on
  `auto-accept-executables` (off by default); a daemon without a terminal declines them. On Windows
  received files get the Mark-of-the-Web (`Zone.Identifier`), so SmartScreen and Office warn
  before they are opened. Received files are never executable on Unix.
- **Safety number**: a 60-digit number shared by two trusted devices that can be compared at any
  time to check that a device is still the one you paired (`ferry peer verify <device>`, the
  *Safety number* item in the GUI device menu, `v` in the TUI). Mark the device as verified
  after comparing; verified devices show ✓. Specified in `docs/PROTOCOL.md` with test vectors.
- **One-time identity for TICKET sharing**: a file shared by TICKET (`ferry send`, *Share* in the
  GUI) is served by a node with a random, throw-away key. The recipient never learns this device's
  permanent Node ID, two shares cannot be linked, and an old TICKET is useless once the share ends.
  The share declines offers sent to it and ends after the first download (CLI) or after 1 hour (GUI).
  Shares listen on UDP 47825–47834, so the firewall rule now covers 47823–47834.
- **Protocol versions are negotiated**: a device lists every version it speaks (`ferry/1` today) and
  the accepting side picks the best common one in the TLS handshake, so a future `ferry/2` can
  be introduced while devices of the previous release still connect. A device with no common version
  gets the clear message "the other device uses an incompatible version – update Ferry on both
  devices" instead of "could not connect". Policy: a release speaks its own and the previous
  version (`docs/PROTOCOL.md`).
- **Protocol specification and test vectors**: `docs/PROTOCOL.md` describes every message,
  the pairing formulas, the discovery datagram, TICKETs and limits;
  `docs/protocol-vectors.json` has exact bytes for fixed keys. A test keeps them equal to the code,
  and `scripts/verify-vectors.py` re-derives them independently from the document alone.

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

- Android release builds: R8 removed or renamed the Kotlin methods that only Rust calls (through
  JNI), so opening received files, the permission card, the background limits and keeping
  transfers alive failed there. They are now kept (`app/proguard-rules.pro`).

- **Browser sessions**: hints for a link that does not load on the phone (same Wi-Fi, Android's
  *Local network* permission for the browser, a VPN on the phone, client isolation in the router)
  under the link in the GUI, in `ferry web` and in the README.
- **Browser sessions**: opening the link on the computer itself (to try it) no longer locks out the
  phone it was meant for, the GUI shows which device opened the link and which were turned away,
  and only addresses of real network interfaces are offered (no Docker or VPN addresses); the
  others wait behind *Does not work? Other addresses*.
- Removing a trusted device now tells it (`Unpair`), so it stops listing the remover as paired
  instead of keeping a pairing the other side no longer honours. A device that is offline is
  remembered for 30 days and told as soon as it appears on the local network (GUI, TUI and daemon);
  an older version keeps the pairing until its user removes it.
- The history of transfers could lose entries when two transfers ended at the same moment (both
  ends of a transfer inside one process, or a file sent to several devices): their lines were
  written in pieces and could be mixed into one damaged line, which was then skipped. Entries are
  now written one at a time, each line in a single write.
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

- The `ferry daemon` systemd service runs sandboxed (system folders read-only, no capabilities,
  restricted system calls and socket types; systemd's exposure score 1.6 instead of 8.6) and
  works for package and cargo installs alike; the .deb and .rpm packages install it. The home
  folder stays writable, so any download folder works; how to restrict it further is in the file.
- The daemon checks at start that it can write to the download folder and says what to fix
  instead of failing on the first file.
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

- **A calmer top bar**: the node status says only *Online*, *Connecting…* or *Offline* (the Node
  ID is in its tooltip and under *My TICKET*), and on phones it shrinks to its coloured dot.
- **The desktop window uses its width**: from 1100 px on, sending and the transfers are on the
  left and the devices on the right, instead of one narrow column in the middle of a large window.
  Narrow windows and phones keep the single column. The README images show the new layout.
- Progress of a transfer reaches the interfaces at most about ten times per second (the first value,
  pauses and the end at once) instead of after every chunk: a 1 GiB transfer on a fast link now
  makes about 30 events on each side instead of about 1260, so the GUI redraws the transfer list
  far less often.
- README: animation, screenshots of the desktop, Android and terminal interfaces, installation per
  platform, a comparison with similar tools, limitations, FAQ and how Ferry is made (with an AI
  coding assistant). The images are made reproducibly from the real interfaces with example data
  (`tests/gui/screenshots.spec.js`, `scripts/make-demo-gif.sh`, `scripts/tui-screenshot.py`).
- A project website (mdBook) made from the README and the documents, with link checking
  (`scripts/build-site.py`, `.github/workflows/site.yml`; published only when started by hand).
  New pages: a detailed comparison with other tools including where Ferry is worse
  (`docs/COMPARISON.md`), measured benchmarks and how to repeat them (`docs/BENCHMARKS.md`), the
  roadmap (`ROADMAP.md`), governance (`GOVERNANCE.md`) and funding (`docs/FUNDING.md`,
  `.github/FUNDING.yml` without accounts yet).
- `sha2` 0.11 (from 0.10), used for the LocalSend certificate fingerprint and the SHA-256 checksums
  that LocalSend senders may include.
- The workspace has three crates: the `ferry` library (no terminal or GUI dependencies), `ferry-cli`
  (`cli/`, the `ferry` binary) and `ferry-gui`. CLI release archives are named `ferry-cli-…`.
- The GUI frontend is split into `index.html`, `styles.css` and ES modules with one translation
  file per language; the Content-Security-Policy no longer allows inline scripts. CI lints it with
  Biome and runs Playwright tests against a mocked Tauri API.
- The terminal UI's state module is split by topic; `rustfmt.toml` documents the formatting.
- The GUI script `app.js` (830 lines) is split into modules by part of the interface (trusted
  devices, nearby devices and pairing, sending, texts, transfers, settings) with shared state in
  `state.js`; behaviour is unchanged.
- Android: `jni` 0.22 (from 0.21). The calls into the Kotlin helpers run inside the new scoped
  thread attachment, which also clears Java exceptions; iroh already used 0.22, so one version less
  is built.
- `tests/network_conditions.rs` prints a progress timeline (bytes sent, packets forwarded, drops,
  ICMP resets, I/O errors) when a test times out, and the QUIC logs with `RUST_LOG`. CI runs the
  outage and network-change tests on Windows in a step allowed to fail, to find why they stop
  there.
- Senders no longer wait for each chunk's acknowledgement; peers must not send chunks smaller than
  16 KiB.

- CI checks formatting without changing files, runs on Linux and Windows, checks the minimum Rust
  version (1.91), the Android build, licenses and advisories (`cargo-deny`) and the GUI frontend.
  Release builds publish an SBOM and build provenance attestations; all actions are pinned to
  commit SHAs and updated by Dependabot.
- Release artifacts are built for Linux, Windows and Android only.

Earlier versions were published privately as "RQS P2P Quick Share" without a changelog.
