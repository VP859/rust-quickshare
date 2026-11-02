# Ferry protocol

This document specifies the protocol of Ferry **`ferry/1`** well enough to write a compatible
implementation. The authoritative source is the code
([`src/protocol/messages.rs`](../src/protocol/messages.rs) and the modules named below);
[`protocol-vectors.json`](protocol-vectors.json) lists exact bytes for fixed inputs and is checked
by a test on every build, so the document, the code and the vectors cannot drift apart unnoticed.

Contents: [versions](#versions-and-negotiation) · [transport and encoding](#transport-and-encoding) ·
[control stream](#control-stream) · [pairing](#pairing) · [discovery](#local-network-discovery) ·
[TICKET](#ticket) · [file names](#file-and-folder-names) · [limits](#limits) ·
[test vectors](#test-vectors) · [changing the protocol](#changing-the-protocol)

## Versions and negotiation

Each protocol version has an ALPN identifier `ferry/<n>` that the two devices exchange in the TLS
handshake (the accepting side picks the best version both list). There is no extra round trip and
an attacker cannot change the choice: ALPN is covered by the handshake's authentication.

* A device lists every version it speaks, newest first
  ([`ProtocolVersion::SUPPORTED`](../src/protocol/version.rs)); `ferry/1` is the only one so far.
* Devices of different releases keep working together as long as they share a version.
* If they share none, the connection is refused with TLS alert 120 (`no_application_protocol`);
  Ferry shows "the other device uses an incompatible version – update both devices".
* The negotiated version is available from the connection (`protocol::version::negotiated`);
  behaviour that differs between versions branches on it.

**Compatibility policy.** A release speaks its own version and the previous one, so one update step
is always possible. A version is dropped only after a release announced it – or immediately when it
has a security flaw, in which case the changelog says so. Negotiation cannot be used to force an
older, weaker version: a version with known weaknesses is removed from `SUPPORTED`, not kept as an
option.

What is **not** a new version: appending a new variant at the *end* of an enum (e.g. a new `Hello`).
A device that does not know it fails to decode the frame and closes that stream; the sender can fall
back (batches did exactly this). Anything else that changes existing bytes is a new version – see
[Changing the protocol](#changing-the-protocol).

## Transport and encoding

* [iroh](https://github.com/n0-computer/iroh) endpoints: QUIC + TLS 1.3, authenticated by the
  Ed25519 **Node ID** (the 32-byte public key; text form is 64 hexadecimal characters). Preferred UDP
  port 47824 (a random one when taken).
* Messages are [postcard](https://postcard.jamesmunns.com/wire-format) values: integers are
  variable-length (LEB128, zig-zag for signed ones), `String` and `Vec` have a length prefix, `[u8; N]`
  is `N` raw bytes, `Option` is a `0`/`1` tag byte, and an **enum is its variant index** (in
  declaration order) followed by the variant's fields. The order of variants and fields is therefore
  part of the protocol.
* A **frame** is a 4-byte big-endian length followed by that many bytes of postcard payload, at
  most 64 KiB ([`codec.rs`](../src/protocol/codec.rs)). A longer announced length is a protocol
  violation, rejected before any buffer is allocated.
* `EndpointAddr` (in `Hello::Pair`, `PairResponse` and TICKETs) is iroh's type and uses its postcard
  encoding; the vectors contain examples.
* One QUIC connection may carry several control streams in turn (e.g. all files of a batch).

## Control stream

The dialing peer opens a bidirectional stream and sends a `Hello` frame. Variant index in brackets:

| Variant | Fields | Meaning |
|---------|--------|---------|
| `Push` [0] | – | a file follows (`TransferOffer`) |
| `Pull` [1] | `token: [u8;16]` | download the file shared under `token` (TICKET) |
| `Pair` [2] | `name: String`, `addr: EndpointAddr`, `commitment: [u8;32]` | pairing request |
| `Text` [3] | `text: String` (≤ 32 KiB) | text for the clipboard |
| `Batch` [4] | `count: u32`, `total_size: u64`, `names: Vec<String>` | several files follow; decide once |

### File transfer (`Push`, and `Pull` in the opposite direction)

```text
sender   ── TransferOffer { file_name, file_size, chunk_size, file_hash, folder } ─▶
receiver ◀─ TransferDecision::Accept { start_chunk_index } | Reject { reason } ─────
sender   ── one unidirectional stream per chunk: ChunkHeader { index, len, hash } + payload ─▶
receiver ◀─ TransferResult::Progress { received }   (every ~250 ms while data arrives) ─
receiver ◀─ TransferResult::Verified | Failed { reason } ───────────────────────────
```

| Message | Fields |
|---------|--------|
| `TransferOffer` | `file_name: String`, `file_size: u64`, `chunk_size: u32`, `file_hash: [u8;32]`, `folder: Vec<String>` |
| `TransferDecision` | `Accept{start_chunk_index: u64}` [0], `Reject{reason: String}` [1] |
| `ChunkHeader` | `chunk_index: u64`, `payload_len: u32`, `chunk_hash: [u8;32]` – followed by `payload_len` raw bytes on the same stream |
| `TransferResult` | `Verified` [0], `Failed{reason: String}` [1], `Progress{received: u64}` [2] |

The sender shows the receiver's `Progress` reports as its own progress: what left the sender's
buffers says little about what arrived on a slow link.

* `chunk_size` depends on the file size (`chunk_size_for`): about 1024 chunks per file, a power of
  two from 1 MiB (files up to 1 GiB) to 8 MiB (above 4 GiB). Since it depends only on the size, a
  resumed transfer uses the same chunks. Receivers accept 16 KiB – 16 MiB.
* Both sides keep `chunk_window(chunk_size)` chunk streams in flight – about 8 MiB of data, at
  least 4 and at most 64 streams – which also bounds the receiver's memory per transfer.
* Before accepting, the receiver checks that the rest of the file fits on its disk with 256 MiB to
  spare; otherwise it declines with a reason (`TransferDecision::Reject`). An accepted batch is
  checked against its announced total size the same way.
* `file_hash` and each `chunk_hash` are BLAKE3 (32 bytes). The receiver verifies every chunk, and
  the whole file at the end, before it renames the `.part` file.
* `folder` lists the folders the file belongs in, relative to the download folder
  (`["Photos", "2024"]`); empty for a single file. At most 16 levels, each sanitized like a file
  name by the receiver.
* `start_chunk_index > 0` resumes from an existing `.part` file.

### Batch

`Hello::Batch` is answered with `BatchDecision::Accepted` [0], `AskEach` [1] or
`Rejected{reason}` [2]. After `Accepted`, the following `Push` streams on the same connection are
received without asking, up to the announced number of files and bytes. `names` (≤ 20, ≤ 255 bytes
each) are for display only.

### Text

`Hello::Text` is answered with `TextDecision::Accepted` [0] or `Rejected{reason}` [1].

## Pairing

```text
requester ── Hello::Pair { name, addr, commitment } ─▶
responder ◀─ PairChallenge { nonce } ─────────────────
requester ── PairReveal { nonce } ───────────────────▶
responder ◀─ PairResponse { accepted, name, addr } ───   after the user decided (≤ 60 s)
```

`PairChallenge` and `PairReveal` carry `nonce: [u8;32]`; `PairResponse` is `accepted: bool`,
`name: String`, `addr: Option<EndpointAddr>` (present only when accepted).

With `BLAKE3-derive-key(context, input)` (BLAKE3's key-derivation mode, 32-byte output), `a` the
requester's Node ID, `b` the responder's, `n_a` and `n_b` their random 32-byte nonces:

```text
commitment = BLAKE3-derive-key("ferry pairing commitment v1", a ‖ n_a)
digest     = BLAKE3-derive-key("ferry pairing code v2",       a ‖ b ‖ n_a ‖ n_b)
number     = (first 8 bytes of digest as little-endian u64) mod 1 000 000
code       = zero-padded 6 digits as "NNN NNN"   (e.g. "709 161")
```

The responder verifies `commitment` against the revealed `n_a` (with `a` from the TLS-authenticated
connection, not from the message) and ends the request without asking the user if they differ. Why
this resists forged codes is in [SECURITY-MODEL.md](SECURITY-MODEL.md#pairing).

## Local network discovery

UDP datagrams on port **47823**, at most 2048 bytes, sent every 5 seconds to the broadcast address of
each local IPv4 network and to the multicast group `239.255.71.83` (TTL 1, also on loopback):

```text
SignedBeacon { magic: [u8;4] = "FRYd", body: Vec<u8> = postcard(Announcement), signature: [u8;64] }
Announcement { timestamp: u64, name: String, platform: String, addr: EndpointAddr,
               quic_ports: Vec<u16> (≤ 4 used), reply: bool }
```

* `signature` is Ed25519 over `"ferry discovery beacon v1" ‖ body` (no separator) with the key of
  `addr.id`; a datagram whose signature does not verify is dropped.
* `timestamp` is seconds since the Unix epoch. Datagrams more than 300 s away from the receiver's
  clock (either direction) are ignored, which limits replays.
* The sender's IP is taken from the packet and combined with each of `quic_ports` to form a LAN
  address, so an announcement works before the sender knows its own addresses.
* A device that has not been heard from for 17 s disappears from the list; at most 256 devices are
  remembered.
* The first announcement from a new device is answered directly (unicast, `reply = true`; replies
  are not answered), so it is enough for packets to get through in one direction. A hidden device
  answers only its paired devices and never announces itself.

## TICKET

`ferry:` followed by the URL-safe base64 (no padding) of `postcard(NodeTicket { addr: EndpointAddr,
share: Option<[u8;16]> })` ([`ticket.rs`](../src/node/ticket.rs)). A TICKET without a token is a
pairing TICKET; with a token it downloads one shared file (`Hello::Pull`). When reading, spaces,
line breaks, quotes and a `ferry://` prefix are tolerated (text wrapped by a terminal).

## File and folder names

A name from a peer is never used as a path. Each file name and each folder component is reduced to
a single safe name: `..`, path separators, absolute paths and empty names are rejected; control
characters and `\ / : * ? " < > | NUL` become `_`; leading and trailing dots and whitespace are
removed; Windows reserved names (`CON`, `NUL`, `COM1`…) get a `_` prefix; the result is joined with
the download folder and checked to stay inside it, symbolic links included. The vectors list inputs
and outputs.

## Limits

| Limit | Value |
|-------|-------|
| Frame payload | 64 KiB |
| File or folder name | 255 bytes |
| Device name | 64 bytes |
| Text | 32 KiB |
| Names in a batch announcement | 20 |
| Folder depth | 16 |
| Chunk size accepted | 16 KiB – 16 MiB (senders use 1 – 8 MiB) |
| Open questions per device / in total | 2 / 8 |
| Announcement age | ±300 s |

Every value received from a peer is validated before use (sizes, counts, names).

## Test vectors

[`protocol-vectors.json`](protocol-vectors.json) contains, for fixed keys (`seed` bytes repeated 32
times → Ed25519 secret key):

* `frames` – the exact bytes of one frame of every message,
* `pairing` – commitment and code for fixed nonces,
* `tickets` – TICKETs with and without address and share token,
* `announcement` – a signed discovery datagram and its validity window,
* `chunk_policy` – chunk size and window for example file sizes,
* `file_names` – inputs and the sanitized names (`null` = rejected).

To check another implementation, encode the same inputs and compare the hex; decode the hex and
compare the values. For Ferry itself the test is `tests/protocol_vectors.rs`
(`cargo test --test protocol_vectors`).

[`scripts/verify-vectors.py`](../scripts/verify-vectors.py) is a second, independent check: about a
hundred lines of Python written only from this document (no Ferry code) reproduce the pairing
commitment and code, the Node IDs, the frame lengths, the TICKET encoding, the chunk-size policy and
the signature of the announcement. CI runs it, so the document stays complete enough to implement.

## Changing the protocol

1. **Is it a new version?** Anything that changes the bytes of an existing message – a field, a
   variant's position, a hash input or context string, a limit that peers must agree on – is. Add
   a variant to `ProtocolVersion` (ALPN `ferry/<n+1>`), make it `CURRENT`, keep the previous one in
   `SUPPORTED` and branch on `negotiated(conn)` where behaviour differs. A new variant at the end of
   an enum is not (see [Versions](#versions-and-negotiation)).
2. Update this document and the changelog (a *Breaking* entry if a version is dropped).
3. Regenerate the vectors with `FERRY_UPDATE_VECTORS=1 cargo test --test protocol_vectors` and read
   the diff: every changed line is a compatibility decision.
4. Add an end-to-end test to `tests/integration_tests.rs` and, for a new version, one to
   `tests/version_negotiation.rs`; run `cargo +nightly fuzz` on the parsers you touched.
