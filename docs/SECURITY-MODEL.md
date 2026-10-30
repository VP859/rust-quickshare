# Security model

This document describes what Ferry protects, against whom, how, and where the limits are. It is
written for users who want to judge whether Ferry fits their needs and for reviewers looking for
weaknesses. Report problems privately as described in [SECURITY.md](../SECURITY.md).

Ferry has not had an external security audit.

## Assets

| Asset | Where it lives |
|-------|----------------|
| Contents of transferred files and texts | in transit between two devices; afterwards in the download folder / clipboard |
| The device key (`secret.key`, Ed25519) – the device's identity | configuration folder, readable only by the user (`0600` on Unix) |
| The list of trusted devices and their Auto-Accept setting | `config.toml` in the configuration folder |
| The user's files outside the download folder | must never be written by a peer |
| The user's attention | dialogs must not be abusable for spam or for tricking the user |

## Attackers considered

1. **Passive observer on the network** (same Wi-Fi, ISP, relay operator in internet mode).
2. **Active attacker on the local network** – can send arbitrary packets, announce fake devices,
   answer instead of a real device, and connect to any Ferry node.
3. **Malicious peer** – a device the user decided to talk to (e.g. accepted a file from), which then
   sends malformed or hostile data.
4. **Thief of a backup or of the configuration folder** – obtains a copy of `secret.key`.

Out of scope: an attacker who controls the user's operating system or a trusted, already paired
device; denial of service by flooding the network itself; side channels on the local machine.

## Protections

### Transport

* Every connection is QUIC with **TLS 1.3** (via [iroh](https://github.com/n0-computer/iroh)).
  Each side proves possession of its Ed25519 key; the **Node ID is the public key**, so a
  connection to Node ID *X* reaches only the holder of *X*'s key. There is no certificate
  authority to trust.
* ALPN `ferry/1` separates Ferry traffic from other iroh applications.
* In the default **local mode** no external server is contacted: no relays, no DNS/pkarr lookup,
  no telemetry. In **internet mode** the public n0 relays may forward encrypted QUIC packets when
  no direct path exists; they see IP addresses, Node IDs and timing, never content.

### Integrity of files

* Every 1 MiB chunk carries its BLAKE3 hash, and the whole file is checked against the hash from
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
* Protocol limits prevent excessive allocation: frames ≤ 64 KiB, chunks ≤ 16 MiB, names ≤ 255
  bytes, texts ≤ 32 KiB, batch announcements ≤ 20 names. An accepted batch cannot carry more
  files or bytes than announced.
* The sender walks folders without following symbolic links, so a link inside a shared folder
  cannot leak files from elsewhere.

### Consent

* Files from devices that are not trusted with **Auto-Accept** require confirmation.
* **Texts** always require confirmation unless the user explicitly enables
  `auto_accept_texts` – even for Auto-Accept devices. A silently replaced clipboard is easy to
  miss and could be pasted unknowingly (e.g. a changed bank account number).
* A daemon without a terminal declines everything that would need a question.
* **Rate limits:** at most 2 open questions per device and 8 in total; requests above the limit
  are declined without a dialog. After a declined or broken pairing request the same device must
  wait 30 s before asking again. Node IDs are free to create, so the global limit is what keeps a
  flood of requests from burying the user in dialogs.

### Pairing

Devices are usually chosen by their announced *name*, which anyone can copy. Pairing therefore
shows a six-digit code on both screens; the user pairs only when they match.

The code is computed from both Node IDs **and two random 32-byte nonces** exchanged with a
commit-reveal scheme (as in Bluetooth Secure Simple Pairing and ZRTP):

1. the requester sends `commitment = BLAKE3-derive-key("ferry pairing commitment v1", requester ID ‖ nonce_a)`,
2. the responder answers with `nonce_b`,
3. the requester reveals `nonce_a`; the responder checks it against the commitment,
4. both compute `code = BLAKE3-derive-key("ferry pairing code v2", requester ID ‖ responder ID ‖ nonce_a ‖ nonce_b) mod 10⁶`.

Neither side can pick its nonce after seeing the other's, so an attacker in the middle cannot
search for keys or nonces that make both screens show the same code: each attempt succeeds with
probability 10⁻⁶ and is visible to the user as an unexpected request. A code derived only from
the two Node IDs (earlier versions of Ferry) could be matched offline in seconds by generating
keys.

### Local network discovery

* Announcements (UDP port 47823) are **signed** with the device key over the payload and a
  timestamp. A device can only announce addresses for its own Node ID, and recorded announcements
  are ignored after 5 minutes (the tolerance allows for clocks that are a few minutes off).
* **Hidden mode** (`hidden = true`, *Settings → Privacy* in the GUI, `H` in the TUI): the device
  does not announce itself and answers only announcements from its paired devices – which are
  verified by their signature. Strangers on the network do not see it; TICKETs and QR codes still
  work for new devices.

### Data at rest

* `secret.key` is created with `0600` permissions on Unix and in the per-user application data
  folder on Windows and Android.
* On Android the app data is excluded from cloud backups and device-to-device transfers
  (`allowBackup="false"` and `data_extraction_rules.xml`), so the key never ends up in a backup.
  A new phone pairs again.
* `ferry identity export <file>` writes a deliberate backup (a new `0600` file with a warning);
  `ferry identity import` restores it and keeps the replaced key as a `.bak` file.
* The key is **not** stored in an OS keyring: Ferry also runs as a headless daemon and on Android,
  where there is no common keyring API, and the key must be available without the user being
  logged in to a desktop session. Full-disk encryption protects it at rest.

### Privacy

* No telemetry, analytics, accounts, crash reports or update checks. Ferry never contacts its
  authors.
* What the local network sees (unless hidden): device name, operating system, Node ID and local
  addresses, every 5 seconds. The device name can be changed at any time.

## Known limitations

* **Announced names are not authenticated.** A stranger can announce the name "Kate's Laptop".
  Before pairing, compare the code; after pairing, trust is bound to the Node ID, not the name.
* **Trust is per device key.** Anyone who obtains `secret.key` (or an export) can pose as that
  device to its paired devices, including Auto-Accept ones. Remove a lost device from the trusted
  list on the other devices.
* **Six-digit codes** give a 10⁻⁶ chance per attempt to an active attacker in the middle; users
  who confirm without comparing the codes are not protected.
* **Replay window:** a recorded announcement can be replayed for up to 5 minutes, e.g. from a
  different IP address. It can only point to the real device (connections are authenticated), so
  the effect is limited to a failed connection attempt to a stale address.
* **Metadata:** in internet mode the relay operator sees who talks to whom and when. In local mode
  anybody on the network sees the announcements and the encrypted traffic between devices.
* **Cancelling** a transfer stops it on this device; the other device sees its streams end and
  reports the transfer as failed. A partial `.part` file stays for a later resume.
* The Android share target receives files from other apps through `content://` URIs; the files are
  copied to the app's private cache before sending, and copies older than a day are deleted at the
  next start.
