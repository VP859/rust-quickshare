# Security model

This document describes what Ferry protects, against whom, how – and, just as importantly, what it
does **not** protect and what other parties can see. It is meant to let you decide whether Ferry
fits your needs without reading the code. Report problems privately as described in
[SECURITY.md](../SECURITY.md).

> **Status:** Ferry is in early development (`0.x`) and has **not** had an external security
> audit. The cryptography comes from established libraries (iroh/QUIC with TLS 1.3, Ed25519,
> BLAKE3); the protocol around it was designed and reviewed only by its authors.

## In short

* File and text **contents** are end-to-end encrypted between the two devices in both network
  modes. No server ever receives them in readable form.
* In **local mode** (the default) Ferry contacts **no server at all**. Everything it sends goes
  to the local network or to a device you chose.
* In **internet mode** Ferry stays connected to a public relay server run by
  [n0](https://n0.computer) and registers its Node ID with n0's lookup service. These servers see
  your IP address, your Node ID, when you are online and whom you exchange data with through the
  relay – but never the content.
* Anyone who knows your **Node ID** can try to contact your device: on the local network in both
  modes, from anywhere in internet mode. They can only *ask* – nothing is accepted without your
  consent unless you enabled Auto-Accept for that exact device.
* There is no telemetry, analytics, account, crash reporting or update check.

## What each party can see

| Party | Local mode | Internet mode (additionally) |
|-------|------------|------------------------------|
| **Devices on the same network** | Unless hidden: your device name, operating system, Node ID, local IP addresses and QUIC ports, every 5 s. Always: that two devices exchange encrypted traffic, its size and timing. | – |
| **A device you send to or receive from** | Your Node ID, device name, IP addresses, the file names, sizes and contents you send it. | – |
| **Someone who knows your Node ID** (e.g. from an old TICKET) | Can connect over the local network and send offers or pairing requests, which you must confirm. | Can do the same **from anywhere on the internet** – your Node ID resolves to your relay. |
| **n0 relay server** | nothing | Your IP address and Node ID for as long as Ferry runs (a permanent connection to your "home relay"); for connections that go through the relay also the other device's Node ID and IP, the amount of data and timing. The relay also tells your device its public IP address. Never content. |
| **n0 lookup service (DNS/pkarr)** | nothing | A signed record "Node ID → home relay URL", republished every 5 minutes (no IP addresses are published), and your IP address from the publishing request. Lookups by other devices reveal which Node ID they are looking for. |
| **Your internet provider / mobile carrier** | nothing (traffic stays in the local network) | That you talk to the n0 relay and DNS servers, and to other IP addresses for direct connections. |
| **Ferry's authors** | nothing | nothing |

A **TICKET** (text or QR code) contains your Node ID and your current IP addresses and ports
(local and, when known, public), and in internet mode your relay URL. Treat it like a phone number:
whoever has it can reach your device and ask for something. A share TICKET additionally lets its
holder download the one shared file while sharing is active.

## Assets

| Asset | Where it lives |
|-------|----------------|
| Contents of transferred files and texts | in transit between two devices; afterwards in the download folder / clipboard |
| The device key (`secret.key`, Ed25519) – the device's identity | configuration folder, readable only by the user (`0600` on Unix) |
| The list of trusted devices and their Auto-Accept setting | `config.toml` in the configuration folder |
| The user's files outside the download folder | must never be read or written by a peer |
| The user's attention | dialogs must not be abusable for spam or for tricking the user |

## Attackers considered

1. **Passive observer** – on the same Wi-Fi, the internet provider, or the relay operator in
   internet mode.
2. **Active attacker on the local network** – sends arbitrary packets, announces fake devices,
   tries to sit between two devices, connects to any Ferry node.
3. **Remote attacker who knows a Node ID** – in internet mode, can reach the device through the
   relay from anywhere.
4. **Malicious peer** – a device you talk to (e.g. you accepted a file from it) that sends
   malformed or hostile data.
5. **Thief of a backup or of the configuration folder** – obtains a copy of `secret.key`.

Out of scope: an attacker who controls your operating system or one of your trusted, already paired
devices; denial of service by flooding the network itself; side channels on the local machine;
compromise of the libraries Ferry depends on (see [Trusted components](#trusted-components)).

## Protections

### Transport

* Every connection is QUIC with **TLS 1.3** (via [iroh](https://github.com/n0-computer/iroh)).
  Each side proves possession of its Ed25519 key; the **Node ID is the public key**, so a
  connection to Node ID *X* reaches only the holder of *X*'s key. There is no certificate
  authority to trust.
* A relay forwards the same end-to-end encrypted QUIC packets; it cannot read or change them
  without the connection failing.
* ALPN `ferry/1` separates Ferry traffic from other iroh applications.
* Local mode uses iroh's `Minimal` preset with relays disabled – no relay, no DNS or pkarr lookup
  and no publishing. Internet mode uses iroh's `N0` preset: the n0 relays plus publishing to and
  resolving from n0's DNS/pkarr service.

### Integrity of files

* Every chunk (1–8 MiB depending on the file size) carries its BLAKE3 hash, and the whole file is checked against the hash from
  the offer before it is renamed from `.part` to its final name. A failed check deletes the
  partial file.
* Resumed downloads re-hash the existing prefix, and the `.part` name includes a hash fragment,
  so data of a different file is never appended.

### File system safety (malicious peer)

* A file name from a peer is never used as a path. It is reduced to a single safe name
  (`..`, separators, absolute paths, control characters and Windows reserved names are rejected
  or replaced) and joined with the download folder; the result is checked to stay inside it.
* Folder transfers send each folder level separately. Every level is sanitized like a file name,
  the depth is limited to 16, and the final folder is resolved through the file system and checked
  again, so an existing symbolic link inside the download folder cannot lead outside.
* A received file never overwrites an existing one (`name (1).ext`).
* Protocol limits prevent excessive allocation: frames ≤ 64 KiB, chunks 16 KiB – 16 MiB, names
  ≤ 255 bytes, texts ≤ 32 KiB, batch announcements ≤ 20 names. The number of chunks read at the
  same time is chosen so that at most about 64 MiB are buffered per transfer, whatever chunk size
  a peer announces. An accepted batch cannot carry more files or bytes than announced.
* **Large files:** before accepting a file or a batch the receiver checks that it fits on the disk
  with 256 MiB to spare and declines otherwise, instead of filling the disk near the end of a long
  transfer. Data goes to a `.part` file and an interrupted transfer resumes from the last complete
  chunk.
* The sender walks folders without following symbolic links, so a link inside a shared folder
  cannot leak files from elsewhere. Peers can never request files: a device only sends what you
  chose to send, or the single file behind a share TICKET you created.

* **Files that can run code** (programs, installers, scripts, shortcuts, macro documents, names with
  direction-changing characters – `ferry::transfer::risk`) are marked with a warning in the offer,
  are never covered by "accept the batch", and are not auto-accepted from Auto-Accept devices
  unless `auto_accept_executables` is on. Received files never get the execute permission; on
  Windows they carry the Mark-of-the-Web (`Zone.Identifier`, zone 3) so SmartScreen and Office
  check them. The list is by extension and is a warning, not a scanner: an unlisted type can still
  be dangerous.

### Consent

* Files from devices that are not trusted with **Auto-Accept** require confirmation. Auto-Accept
  is off by default and set per device.
* **Texts** always require confirmation unless you explicitly enable `auto_accept_texts` – even
  for Auto-Accept devices. A silently replaced clipboard is easy to miss and could be pasted
  unknowingly (e.g. a changed bank account number).
* A daemon without a terminal declines everything that would need a question.
* **Resuming after a broken connection** does not ask again: when the user accepted a file (or a
  batch) and its connection broke, the same device – identified by its key – may continue that
  exact file (same BLAKE3 hash) into the same folder, and the rest of the batch, within 10 minutes.
  Nothing else is accepted on that basis, and at most 256 such answers are remembered.
* **Rate limits:** at most 2 open questions per device and 8 in total; requests above the limit
  are declined without a dialog. After a declined or broken pairing request the same device must
  wait 30 s before asking again. Node IDs are free to create, so the global limit is what keeps a
  flood of requests from burying you in dialogs.

### Pairing

Devices are usually chosen by their announced *name*, which anyone can copy. Pairing therefore
shows a six-digit code on both screens; pair only when they match.

The code is computed from both Node IDs **and two random 32-byte nonces** exchanged with a
commit-reveal scheme (as in Bluetooth Secure Simple Pairing and ZRTP):

1. the requester sends `commitment = BLAKE3-derive-key("ferry pairing commitment v1", requester ID ‖ nonce_a)`,
2. the responder answers with `nonce_b`,
3. the requester reveals `nonce_a`; the responder checks it against the commitment,
4. both compute `code = BLAKE3-derive-key("ferry pairing code v2", requester ID ‖ responder ID ‖ nonce_a ‖ nonce_b) mod 10⁶`.

Neither side can pick its nonce after seeing the other's, so an attacker in the middle cannot
search for keys or nonces that make both screens show the same code: each attempt succeeds with
probability 10⁻⁶ and is visible to you as an unexpected request. A code derived only from the two
Node IDs (earlier versions of Ferry) could be matched offline in seconds by generating keys.

### Local network discovery

* Announcements (UDP port 47823) are **signed** with the device key over the payload and a
  timestamp. A device can only announce addresses for its own Node ID, and recorded announcements
  are ignored after 5 minutes (the tolerance allows for clocks that are a few minutes off).
* **Hidden mode** (`hidden = true`, *Settings → Privacy* in the GUI, `H` in the TUI): the device
  does not announce itself and answers only announcements from its paired devices – which are
  verified by their signature. Strangers on the local network do not see it in their lists.

  Hidden mode is about **local network discovery only**. It does not make the device unreachable:
  someone who already knows its Node ID or has an old TICKET can still connect and ask, and in
  internet mode the Node ID stays registered with n0's lookup service and reachable through the
  relay. The device keeps listening on UDP 47823 and 47824, so a network scan can still notice
  that something answers there.

### Data at rest

* `secret.key` is created with `0600` permissions on Unix and in the per-user application data
  folder on Windows and Android. It is not encrypted with a password; full-disk encryption
  protects it at rest.
* On Android the app data is excluded from cloud backups and device-to-device transfers
  (`allowBackup="false"` and `data_extraction_rules.xml`), so the key never ends up in a backup.
  A new phone pairs again.
* `ferry identity export <file>` writes a deliberate backup (a new `0600` file with a warning);
  `ferry identity import` restores it and keeps the replaced key as a `.bak` file.
* The key is **not** stored in an OS keyring: Ferry also runs as a headless daemon and on Android,
  where there is no common keyring API, and the key must be available without a desktop login.
* Received files are stored unencrypted in the download folder (on Android `Download/Ferry`;
  photos and videos in `Pictures/Ferry`, visible to the gallery and to apps with media access).
* Ferry does not write log files itself. Logs go to the terminal (CLI, daemon) or to the system
  log (Android logcat); at the default level they contain Node IDs, device names, file names and
  paths.
* System notifications show the sender's name, the file name or number of files, and the pairing
  code. On a phone they may be visible on the lock screen depending on the system settings.

### Safety number

A trusted device is bound to its key, but the six-digit pairing code is seen only once. The
**safety number** (60 digits, `ferry peer verify`) is derived from both Node IDs and is the same
on both devices at any time; reading it out or comparing it side by side shows that both still
see the same two keys, i.e. nobody replaced a device or sits in between. 60 digits (about 200
bits) cannot be matched by generating keys. It proves key identity only: it says nothing about
who holds the device. Verification is a local mark and is not synchronized.

### Sharing by TICKET

A file shared by TICKET is served by a node with a random one-time key, not the device's own.
The recipient therefore cannot learn or track the permanent Node ID, shares cannot be linked
to each other or to the device, and the node disappears when the share ends (first download,
timeout or stop). The share serves one file and declines offers. Not hidden: the sender's and
recipient's IP addresses (visible to each other, and the relay in internet mode), and anyone
who gets the TICKET while the share is open can download the file once or until it expires.

## Known limitations

* **Announced names are not authenticated.** A stranger can announce the name "Kate's Laptop".
  Before pairing, compare the code; after pairing, trust is bound to the Node ID, not the name.
* **The Node ID is a stable identifier** (except in TICKET shares, which use a one-time one). It does not change between networks or restarts, so
  anyone who sees it (local network, relay operator, a TICKET you shared) can recognize the same
  device later. A new identity is possible only by deleting `secret.key`, which also loses all
  pairings.
* **Trust is per device key.** Anyone who obtains `secret.key` (or an export) can pose as that
  device to its paired devices, including Auto-Accept ones. Remove a lost device from the trusted
  list on the other devices.
* **Auto-Accept** means files from that device are saved without asking. If that device is
  compromised, it can fill your download folder.
* **Six-digit codes** give a 10⁻⁶ chance per attempt to an active attacker in the middle; if you
  confirm without comparing the codes, you are not protected.
* **Replay window:** a recorded announcement can be replayed for up to 5 minutes, e.g. from a
  different IP address. It can only point to the real device (connections are authenticated), so
  the effect is limited to a failed connection attempt to a stale address.
* **Metadata** is not hidden: who talks to whom, when and how much is visible to the local network
  and, in internet mode, to the relay operator and your provider. Ferry offers no anonymity.
* **File names are visible to the receiver before accepting**, and in a batch up to 20 names are
  shown. They are encrypted in transit like the content.
* **Cancelling** a transfer stops it on this device; the other device sees its streams end and
  reports the transfer as failed. A partial `.part` file stays for a later resume.
* The Android share target receives files from other apps through `content://` URIs; the files are
  copied to the app's private cache before sending, and copies older than a day are deleted at the
  next start.

## Trusted components

Ferry relies on, and is only as secure as:

* [iroh](https://github.com/n0-computer/iroh) and its QUIC implementation for connections, TLS
  and key handling,
* [rustls](https://github.com/rustls/rustls) and its crypto provider, `ed25519-dalek`, `blake3`,
* [Tauri](https://tauri.app) and the system web view (WebKitGTK, WebView2, Android WebView) for
  the GUI,
* in internet mode, the availability and honesty of n0's relay and lookup servers (they can deny
  service or log metadata, but cannot read or forge content).

Dependencies are checked for known vulnerabilities and license problems in CI (`cargo-deny`) and
updated by Dependabot.
