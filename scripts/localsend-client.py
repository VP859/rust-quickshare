#!/usr/bin/env python3
"""A minimal LocalSend protocol v2 client, written from the specification
(https://github.com/localsend/protocol) without Ferry or LocalSend code.

It does what the LocalSend app on an iPhone or a Mac does when it sends files,
so the LocalSend compatibility mode of Ferry can be tested from any computer
with Python 3.9+ and no extra packages:

    python3 scripts/localsend-client.py discover
    python3 scripts/localsend-client.py info 192.168.1.20
    python3 scripts/localsend-client.py send 192.168.1.20 photo.jpg notes.txt Folder/

Like the official client it pins the receiver's certificate: the SHA-256 of
the certificate must equal the fingerprint the receiver announces.
"""

import argparse
import hashlib
import http.client
import json
import os
import socket
import ssl
import struct
import sys
import time
import uuid

PORT = 53317
GROUP = "224.0.0.167"
API = "/api/localsend/v2"
FINGERPRINT = uuid.uuid4().hex.upper()  # This client has no certificate.


def own_info(announce=None):
    info = {
        "alias": f"Test client ({socket.gethostname()})",
        "version": "2.1",
        "deviceModel": "Python",
        "deviceType": "headless",
        "fingerprint": FINGERPRINT,
        "port": PORT,
        "protocol": "https",
        "download": False,
    }
    if announce is not None:
        info["announce"] = announce
    return info


class Pinned(http.client.HTTPSConnection):
    """HTTPS connection that records the server certificate's fingerprint and
    refuses a different one than `expected` (when given)."""

    def __init__(self, host, port, expected=None, timeout=None):
        context = ssl.create_default_context()
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE  # self-signed; pinned below
        super().__init__(host, port, context=context, timeout=timeout)
        self.expected = expected
        self.fingerprint = None

    def connect(self):
        super().connect()
        der = self.sock.getpeercert(binary_form=True)
        self.fingerprint = hashlib.sha256(der).hexdigest().upper()
        if self.expected and self.fingerprint != self.expected.upper():
            self.close()
            raise ssl.SSLError(f"certificate fingerprint {self.fingerprint} is not the pinned {self.expected}")


def request(host, port, method, path, body=None, pin=None, timeout=None):
    conn = Pinned(host, port, pin, timeout)
    headers = {"Content-Type": "application/json"} if isinstance(body, (dict, list)) else {}
    data = json.dumps(body).encode() if isinstance(body, (dict, list)) else body
    conn.request(method, API + path, body=data, headers=headers)
    response = conn.getresponse()
    payload = response.read()
    conn.close()
    try:
        payload = json.loads(payload) if payload else None
    except ValueError:
        payload = payload.decode(errors="replace")
    return response.status, payload, conn.fingerprint


def cmd_discover(args):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if hasattr(socket, "SO_REUSEPORT"):
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    sock.bind(("", PORT))
    membership = struct.pack("4s4s", socket.inet_aton(GROUP), socket.inet_aton("0.0.0.0"))
    sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, membership)
    sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
    sock.settimeout(0.5)
    sock.sendto(json.dumps(own_info(announce=True)).encode(), (GROUP, PORT))
    print(f"Announced on {GROUP}:{PORT}, listening for {args.seconds} s…")
    seen = {}
    end = time.time() + args.seconds
    while time.time() < end:
        try:
            data, (ip, _) = sock.recvfrom(65536)
        except socket.timeout:
            continue
        try:
            device = json.loads(data)
        except ValueError:
            continue
        if device.get("fingerprint") == FINGERPRINT or device.get("fingerprint") in seen:
            continue
        seen[device.get("fingerprint")] = device
        print(f"  {device.get('alias')!r} at {ip}:{device.get('port')} "
              f"({device.get('deviceModel')}, {device.get('protocol')}), "
              f"fingerprint {device.get('fingerprint')}")
    if not seen:
        print("No device answered (is multicast blocked by a firewall?)")
    return 0 if seen else 1


def discover_by_address(host, port):
    """`register` without a pin, as the official client does for an address;
    returns the device info and the certificate fingerprint."""
    status, info, fingerprint = request(host, port, "POST", "/register", own_info(), timeout=5)
    if status != 200:
        sys.exit(f"register answered {status}: {info}")
    if info.get("fingerprint") and info["fingerprint"] != fingerprint:
        sys.exit(f"announced fingerprint {info['fingerprint']} differs from the certificate {fingerprint}")
    return info, fingerprint


def cmd_info(args):
    info, fingerprint = discover_by_address(args.host, args.port)
    print(json.dumps(info, indent=2))
    print(f"certificate fingerprint: {fingerprint}")
    return 0


def collect(paths):
    """(relative name with '/', absolute path) for files and folder contents."""
    for path in paths:
        path = os.path.abspath(path)
        if os.path.isdir(path):
            root = os.path.dirname(path.rstrip(os.sep))
            for folder, _, files in os.walk(path):
                for name in sorted(files):
                    full = os.path.join(folder, name)
                    yield os.path.relpath(full, root).replace(os.sep, "/"), full
        else:
            yield os.path.basename(path), path


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def cmd_send(args):
    info, pin = discover_by_address(args.host, args.port)
    print(f"Receiver: {info.get('alias')!r} ({info.get('deviceModel')}), fingerprint {pin}")
    files, paths = {}, {}
    for name, path in collect(args.paths):
        file_id = uuid.uuid4().hex
        files[file_id] = {
            "id": file_id, "fileName": name, "size": os.path.getsize(path),
            "fileType": "application/octet-stream",
            "sha256": None if args.no_hash else sha256_of(path), "preview": None,
        }
        paths[file_id] = path
    if not files:
        sys.exit("nothing to send")
    print(f"Offering {len(files)} file(s); accept on the receiver…")
    status, answer, _ = request(args.host, args.port, "POST", "/prepare-upload",
                                {"info": own_info(), "files": files}, pin=pin)
    if status == 204:
        print("Receiver needs nothing (204)")
        return 0
    if status != 200:
        print(f"prepare-upload: {status} {answer}")
        return 1
    session, tokens = answer["sessionId"], answer["files"]
    failed = 0
    for file_id, token in sorted(tokens.items(), key=lambda t: files[t[0]]["fileName"]):
        with open(paths[file_id], "rb") as f:
            data = f.read()
        query = f"/upload?sessionId={session}&fileId={file_id}&token={token}"
        status, answer, _ = request(args.host, args.port, "POST", query, data, pin=pin)
        print(f"  {files[file_id]['fileName']}: {status}{'' if status == 200 else ' ' + str(answer)}")
        failed += status != 200
    return 1 if failed else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("discover", help="announce and list LocalSend devices answering")
    p.add_argument("--seconds", type=float, default=4)
    p.set_defaults(func=cmd_discover)
    for name, func in (("info", cmd_info), ("send", cmd_send)):
        p = sub.add_parser(name)
        p.add_argument("host")
        p.add_argument("--port", type=int, default=PORT)
        if name == "send":
            p.add_argument("paths", nargs="+")
            p.add_argument("--no-hash", action="store_true", help="do not send SHA-256 checksums")
        p.set_defaults(func=func)
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
