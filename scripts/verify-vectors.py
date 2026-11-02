#!/usr/bin/env python3
"""Checks docs/protocol-vectors.json with an implementation written only from
docs/PROTOCOL.md – no Ferry code. If this passes, the specification is enough
to build a compatible implementation of the parts it covers.

Needs the packages `blake3` and `cryptography`:
    python3 -m venv /tmp/v && /tmp/v/bin/pip install blake3 cryptography
    /tmp/v/bin/python scripts/verify-vectors.py
"""

import base64
import json
import pathlib
import sys

from blake3 import blake3
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)

VECTORS = pathlib.Path(__file__).resolve().parent.parent / "docs" / "protocol-vectors.json"


def derive_key(context: str, data: bytes) -> bytes:
    """BLAKE3 in key-derivation mode with a context string (32-byte output)."""
    return blake3(data, derive_key_context=context).digest()


def read_varint(buf: bytes, i: int) -> tuple[int, int]:
    """LEB128 as used by postcard: returns the value and the next position."""
    result = shift = 0
    while True:
        byte = buf[i]
        i += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            return result, i


def check_keys(v: dict) -> None:
    for name in ("1", "2"):
        seed = bytes.fromhex(v["keys"][name]["seed"])
        public = Ed25519PrivateKey.from_private_bytes(seed).public_key().public_bytes_raw()
        assert public.hex() == v["keys"][name]["node_id"], f"Node ID of key {name}"


def check_frames(v: dict) -> None:
    for frame in v["frames"]:
        raw = bytes.fromhex(frame["frame"])
        length = int.from_bytes(raw[:4], "big")
        assert length == len(raw) - 4 == frame["payload_length"], frame["name"]
        assert length <= 64 * 1024, frame["name"]


def check_pairing(v: dict) -> None:
    p = v["pairing"]
    a, b = bytes.fromhex(p["initiator_node_id"]), bytes.fromhex(p["responder_node_id"])
    n_a, n_b = bytes.fromhex(p["initiator_nonce"]), bytes.fromhex(p["responder_nonce"])
    commitment = derive_key("ferry pairing commitment v1", a + n_a)
    assert commitment.hex() == p["commitment"], "commitment"
    digest = derive_key("ferry pairing code v2", a + b + n_a + n_b)
    number = int.from_bytes(digest[:8], "little") % 1_000_000
    assert f"{number // 1000:03d} {number % 1000:03d}" == p["code"], "pairing code"


def check_safety_number(v: dict) -> None:
    s = v["safety_number"]
    a, b = bytes.fromhex(s["node_id_a"]), bytes.fromhex(s["node_id_b"])
    lo, hi = sorted([a, b])
    stream = blake3(lo + hi, derive_key_context="ferry safety number v1").digest(length=96)
    groups = [
        f"{int.from_bytes(stream[i:i + 8], 'little') % 100_000:05d}"
        for i in range(0, 96, 8)
    ]
    assert " ".join(groups) == s["number"], "safety number"
    assert s["number"] == s["number_swapped"], "symmetric"


def check_tickets(v: dict) -> None:
    for ticket in v["tickets"]:
        text = ticket["ticket"]
        assert text.startswith("ferry:"), ticket["name"]
        payload = text[len("ferry:"):]
        raw = base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4))
        assert raw[:32] == bytes.fromhex(v["keys"]["1"]["node_id"]), ticket["name"]


def check_announcement(v: dict) -> None:
    a = v["announcement"]
    datagram = bytes.fromhex(a["datagram"])
    assert len(datagram) <= 2048 and datagram[:4] == b"FRYd"
    body_len, i = read_varint(datagram, 4)
    body, signature = datagram[i:i + body_len], datagram[i + body_len:]
    assert len(signature) == 64, "signature is 64 raw bytes"
    Ed25519PublicKey.from_public_bytes(bytes.fromhex(a["node_id"])).verify(
        signature, b"ferry discovery beacon v1" + body
    )
    timestamp, _ = read_varint(body, 0)
    assert timestamp == a["timestamp"]
    assert a["valid_until"] - a["valid_from"] == 600, "accepted: timestamp ± 300 s"


def check_chunk_policy(v: dict) -> None:
    mib = 1 << 20
    for row in v["chunk_policy"]:
        wanted = max(mib, min(8 * mib, -(-row["file_size"] // 1024)))
        chunk = 1 << (wanted - 1).bit_length()  # round up to a power of two
        assert chunk == row["chunk_size"], row
        assert row["window"] == max(4, min(64, (8 * mib) // chunk)), row


def main() -> int:
    vectors = json.loads(VECTORS.read_text(encoding="utf-8"))
    checks = [check_keys, check_frames, check_pairing, check_safety_number, check_tickets,
              check_announcement, check_chunk_policy]
    for check in checks:
        check(vectors)
        print(f"ok  {check.__name__[len('check_'):]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
