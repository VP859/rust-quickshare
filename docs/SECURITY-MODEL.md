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

In the **own relay mode** (`relay`) the n0 rows are replaced by your relay: it sees what the n0
relay would (IP addresses, Node IDs, amounts and timing, never content), and nothing is published to
or looked up from n0's DNS. Paired devices are reached at the address saved when pairing (which
contains their relay URL), so a device whose relay changed must be paired again. Plain `http://`
relay URLs are refused except for IP addresses on this computer or the local network, because they
would show the Node IDs to everyone on the way.

A **TICKET** (text or QR code) contains your Node ID and your current IP addresses and ports
(local and, when known, public), and in internet or relay mode your relay URL. Treat it like a phone number:
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
* **Only paired devices** (`paired-only`, *Settings → Privacy*, off by default): offers, batches
  and texts from a device that is not paired are declined at once with "accepts files only from
  paired devices"; the user sees nothing and nothing is asked. Pairing requests from unknown
  devices get no answer, except while **pairing is open** (`ferry pairing`, a button in the GUI;
  5 minutes by default, at most 30, then it closes by itself) – the code is still compared and
  confirmed as usual. Pairing started *from* this device always works. This closes the gap that
  anyone who knows your Node ID or has an old TICKET can ask: they can no longer even do that.
  It does not hide the device (see hidden mode), does not cover downloads of a shared file by
  its TICKET (the token is the secret), and does not apply to the LocalSend mode and browser
  sessions, which have their own switches and always ask.
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
  relay. The device keeps listening on UDP 47823 and 47824 (or a random port, see below), so a network
  scan can still notice that something answers there.
* **Sealed answers:** pairing gives both devices a secret key that nobody else can compute. A
  hidden device answers a paired device in a packet encrypted and authenticated with it
  ([PROTOCOL.md](PROTOCOL.md#sealed-answers-of-hidden-devices)), so someone who listens on the
  Wi-Fi sees no name, no Node ID, no operating system and no Ferry marker – only that some packet
  went from one address to another (and the announcement of the paired device, if that one is
  visible). Devices paired before this existed are answered in the clear until they are paired
  again. The secret is kept in `config.toml`, which only its owner can read.
* **The operating system** is not put in announcements unless you choose to (`announce-platform`);
  it only picks an icon on other devices. Paired devices get it in sealed answers.
* **Random port** (`random-port`, off by default): direct connections listen on a random UDP port
  instead of 47824, so a scan for that port finds nothing. Other devices learn the port from
  announcements and TICKETs. Discovery on 47823 stays (visible devices need it) and a firewall must
  allow the program instead of a port, so this is a small gain, not invisibility.

### Data at rest

* `config.toml` is written with `0600` permissions on Unix, because it holds the secrets shared with
  paired devices (for sealed discovery answers). They reveal that a hidden device is nearby, and
  nothing more: files and connections do not depend on them.
* `secret.key` is created with `0600` permissions on Unix and in the per-user application data
  folder on Windows and Android. It is not encrypted with a password; full-disk encryption
  protects it at rest.
* On Android the app data is excluded from cloud backups and device-to-device transfers
  (`allowBackup="false"` and `data_extraction_rules.xml`), so the key never ends up in a backup.
  A new phone pairs again.
* `ferry identity export <file>` writes a deliberate backup (a new `0600` file with a warning);
  `ferry identity import` restores it and keeps the replaced key as a `.bak` file.
* **Optionally**, on desktop Linux and Windows, the key can be kept in the system's secret store
  instead (`ferry identity storage system`, *Settings → Privacy*): Secret Service (KWallet, GNOME
  Keyring) or the Credential Manager, protected by DPAPI. It is then encrypted while the user is
  logged out and is not in copies or backups of the configuration folder. It is **not** hidden from
  other programs of the same user, who can ask the store for it like Ferry does. The key is
  written to the store and read back before the file is overwritten and removed; an interruption
  leaves a working key in one place, and Ferry never generates a new identity when the store is
  unreachable – it stops with an error instead. A daemon started without a desktop session cannot
  read the store, so the file is the default.
* Received files are stored unencrypted in the download folder (on Android `Download/Ferry`;
  photos and videos in `Pictures/Ferry`, visible to the gallery and to apps with media access).
* The **history of transfers** (`history.jsonl` in the configuration folder, `0600` on Unix) holds
  the file names, sizes, devices, times, save paths, paths of sent files (to send them again) and
  error messages of the last 1000 finished
  transfers – no content. Texts and links are listed too, but only that one was sent or received
  and its size, never what it said (it may be a password or an account number). It never leaves the device. `ferry history --clear` or *Clear history*
  deletes it; `history = false` (*Settings → Privacy*) stops recording.
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

### LocalSend compatibility mode

An optional mode (`localsend` setting, off by default; `src/localsend/`) speaks LocalSend protocol
v2 as a receiver so that devices without Ferry – above all iPhones and Macs – can send files. It
deliberately runs with **LocalSend's** security model, which is weaker than Ferry's:

* **No authentication of the sender.** LocalSend has no pairing; a sender identifies itself only by
  a self-chosen alias. Anyone on the local network can offer files, under any name. Ferry does not
  request client certificates, so it cannot tell two LocalSend devices with the same name apart.
* **What is protected:** the connection uses TLS (HTTPS with a self-signed ECDSA certificate kept in
  the configuration folder); LocalSend apps pin its SHA-256 fingerprint, learned from the multicast
  announcement, so on a network where the announcement arrives unmodified a passive listener cannot
  read the files. An active attacker on the local network can announce its own certificate.
* **Mitigations in Ferry:** every offer is asked about (never Auto-Accept, not even for files from
  a paired device's address; a daemon without a terminal declines), the dialog says *LocalSend · not
  verified* and shows the sender's IP address, the risky-file warning covers every offered file (not
  only the listed names), one session at a time, an address must wait 15 s after a declined offer,
  at most 10 000 files and 64 connections, request bodies are size-limited, every path component is
  sanitized like Ferry file names (folders are kept, `..` and absolute paths are refused), a file
  must arrive with exactly its announced size and, when the sender gives one, its SHA-256 – otherwise
  nothing is kept – and saved files get the Mark-of-the-Web on Windows.
* **Separate identity:** the LocalSend certificate is not derived from the Ferry key; LocalSend
  devices never see the Node ID and cannot link the device to its Ferry identity (except through
  the announced name and the IP address).
* **Sending to LocalSend apps** (`src/localsend/client.rs`, without the mode) has the mirror
  weakness: the **receiver is not authenticated**. A LocalSend app is known by what it announces
  on multicast (or by the address the user typed); anyone on the local network can announce any
  name. Ferry pins the certificate fingerprint of the announcement, so the connection goes to the
  device that announced it and not to someone who only answers at the same address, and refuses
  to send when the certificate differs – but it cannot know *who* announced it. The interfaces say
  so before sending. Searching (`ferry localsend scan`, *Search for LocalSend apps*) announces this
  device's name and the compatibility-mode certificate fingerprint to every LocalSend app on the
  network; it happens only when the user asks, and for its few seconds a server on TCP 53317
  answers only `register` and `info` (when the compatibility mode is off). An address typed by the
  user is trusted on first use: its certificate is taken as it comes. Ferry presents the
  compatibility mode's certificate as a TLS client certificate, as newer LocalSend apps require;
  it is the one whose fingerprint the offer names, not the Ferry key. A PIN, for apps that require
  one, is sent in the query of the encrypted request, as the official app does.
* The LocalSend download API (sharing through a browser) is not offered.

### Browser sessions

`ferry web` and *Receive from a browser* (`src/web/`) start an HTTP server on TCP 47835 for one
session, so a device without Ferry can download chosen files and send files with a web browser.

* **No encryption.** Browsers reject self-signed certificates without a warning page, and teaching
  users to click through it is worse. A passive attacker on the local network can read the files and
  the token. Use sessions only on trusted networks; Ferry says so in the CLI, the GUI and on the page.
* **Access:** a 128-bit random token in the path (compared in constant time); every other path is
  `404` and says nothing about Ferry. The first address that uses the token owns the session, and
  other addresses are refused, so a token seen later (network capture, photo of the QR code) is
  useless. An attacker who forges that address on the local network is not stopped by this.
* **Lifetime:** 30 minutes by default, or until stopped; the server closes with the session.
* **Receiving:** every batch is asked about in Ferry, never accepted automatically. The same
  file-name sanitizing, free-space and size checks, risky-file warnings and Mark-of-the-Web apply as
  for other transfers. One batch at a time, at most 1000 files, and a 15 s wait after a declined
  batch.
* **Sending:** only the files chosen for the session can be downloaded, always as `attachment`
  with `application/octet-stream`, so a shared HTML file never runs in the browser.
* **Page:** served with a strict Content-Security-Policy (`default-src 'none'`, scripts and styles
  only from the page itself, no frames), `no-store`, `nosniff` and `no-referrer`.

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
* **Node ID as an address:** without `paired-only`, anyone who knows your Node ID can send
  offers and pairing requests (you must still confirm). Switch it on to refuse them silently.
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
  reports the transfer as failed. A partial `.part` file stays for a later resume, unless the
  download was cancelled with *Cancel and delete* (`cancel_and_discard`), which removes it. Files
  from LocalSend apps and browsers are never kept partly: their `.part` files go when the upload ends early.
* The Android share target receives files from other apps through `content://` URIs; the files are
  copied to the app's private cache before sending, and copies older than a day are deleted at the
  next start.

## Trusted components

Ferry relies on, and is only as secure as:

* [iroh](https://github.com/n0-computer/iroh) and its QUIC implementation for connections, TLS
  and key handling,
* [rustls](https://github.com/rustls/rustls) and its crypto provider, `ed25519-dalek`, `blake3`,
* with the LocalSend mode on: [hyper](https://hyper.rs) (HTTP/1.1 server), `rcgen` (certificate)
  and `sha2`,
* [Tauri](https://tauri.app) and the system web view (WebKitGTK, WebView2, Android WebView) for
  the GUI,
* in internet mode, the availability and honesty of n0's relay and lookup servers (they can deny
  service or log metadata, but cannot read or forge content).

Dependencies are checked for known vulnerabilities and license problems in CI (`cargo-deny`) and
updated by Dependabot.
