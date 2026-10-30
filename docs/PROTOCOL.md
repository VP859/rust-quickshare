# Ferry protocol

Version: ALPN **`ferry/1`**. An incompatible change to any message below bumps the ALPN, so
devices running different protocol versions refuse to connect instead of misreading each other.

All types live in [`src/protocol/messages.rs`](../src/protocol/messages.rs); the API documentation
(`cargo doc --open`) describes every field.

## Transport

* [iroh](https://github.com/n0-computer/iroh) endpoints: QUIC + TLS 1.3, authenticated by the
  Ed25519 Node ID. Preferred UDP port 47824 (random when taken).
* Frames: [postcard](https://docs.rs/postcard) with a 4-byte big-endian length prefix, at most
  64 KiB ([`codec.rs`](../src/protocol/codec.rs)).
* One QUIC connection may carry several control streams in turn (e.g. all files of a batch).

## Control stream

The dialing peer opens a bidirectional stream and sends a `Hello`:

```text
Hello::Push                                  a file follows (TransferOffer)
Hello::Pull  { token }                       download the file shared under `token` (TICKET)
Hello::Pair  { name, addr, commitment }      pairing request
Hello::Text  { text }                        text for the clipboard (≤ 32 KiB)
Hello::Batch { count, total_size, names }    several files follow, decide once
```

### File transfer (`Push`, and `Pull` in the opposite direction)

```text
sender   ── TransferOffer { file_name, file_size, chunk_size, file_hash, folder } ─▶
receiver ◀─ TransferDecision::Accept { start_chunk_index } | Reject { reason } ─────
sender   ── one unidirectional stream per chunk: ChunkHeader { index, len, hash } + payload ─▶
receiver ◀─ TransferResult::Verified | Failed { reason } ───────────────────────────
```

* `chunk_size` is 1 MiB (accepted: 1 B – 16 MiB); up to 8 chunk streams are in flight.
* `file_hash` and each chunk `hash` are BLAKE3.
* `folder` lists the folders the file belongs in, relative to the download folder
  (`["Photos", "2024"]`); empty for a single file. At most 16 levels, each sanitized like a file
  name by the receiver.
* `start_chunk_index > 0` resumes from an existing `.part` file.

### Batch

`Hello::Batch` is answered with `BatchDecision::Accepted`, `AskEach` or `Rejected { reason }`.
After `Accepted`, the following `Push` streams on the same connection are received without asking,
up to the announced number of files and bytes. `names` (≤ 20, ≤ 255 bytes each) are for display
only.

### Text

`Hello::Text` is answered with `TextDecision::Accepted` or `Rejected { reason }`.

### Pairing

```text
requester ── Hello::Pair { name, addr, commitment } ─▶
responder ◀─ PairChallenge { nonce } ─────────────────
requester ── PairReveal { nonce } ───────────────────▶
responder ◀─ PairResponse { accepted, name, addr } ───   after the user decided (≤ 60 s)
```

`commitment` and the six-digit code are defined in
[SECURITY-MODEL.md](SECURITY-MODEL.md#pairing). A reveal that does not match the commitment ends
the request without asking the user.

## Local network discovery

UDP datagrams on port **47823**, sent every 5 seconds to the broadcast address of each local IPv4
network and to the multicast group `239.255.71.83` (TTL 1, also on loopback):

```text
SignedBeacon { magic: "FRYd", body: postcard(Beacon), signature }
Beacon       { timestamp, name, platform, addr, quic_ports, reply }
```

* `signature` is Ed25519 over `"ferry discovery beacon v1" ‖ body` with the key of `addr.id`.
* Beacons more than 300 s away from the receiver's clock are ignored.
* The first beacon from a new device is answered directly (unicast, `reply = true`), so it is
  enough for packets to get through in one direction. Hidden devices answer only paired devices
  and never announce themselves.

## TICKET

`ferry:` followed by URL-safe base64 of the postcard-encoded endpoint address and an optional
16-byte share token ([`ticket.rs`](../src/node/ticket.rs)). A TICKET without a token is a pairing
TICKET; with a token it downloads one shared file (`Hello::Pull`).

## Changing the protocol

* A new `Hello` variant or message changes the postcard encoding – bump the ALPN
  (`ferry/2`), update this document and add an end-to-end test in
  `tests/integration_tests.rs`.
* Every value received from a peer must be validated before use (sizes, counts, names).
