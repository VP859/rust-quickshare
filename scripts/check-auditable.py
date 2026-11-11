#!/usr/bin/env python3
"""Checks that release binaries carry their dependency list (cargo auditable).

`cargo auditable` puts a zlib-compressed JSON list of the crates (names,
versions, sources) into a section called `.dep-v0` of the binary (ELF on
Linux, PE on Windows), which `cargo audit bin`, Syft, Trivy and others read.
This script reads that section and fails when a binary lacks it, so a release
never ships without it by accident.

Usage: python3 -I scripts/check-auditable.py BINARY...
"""
import json
import struct
import sys
import zlib

SECTION = ".dep-v0"


def elf_section(data: bytes, name: str):
    """The bytes of section `name` of a 64-bit little-endian ELF file, or None."""
    if data[4] != 2 or data[5] != 1:
        raise ValueError("only 64-bit little-endian ELF files are supported")
    shoff = struct.unpack_from("<Q", data, 0x28)[0]
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x3A)

    def header(i):
        # sh_name, sh_type, sh_flags, sh_addr, sh_offset, sh_size
        return struct.unpack_from("<IIQQQQ", data, shoff + i * shentsize)

    names_offset = header(shstrndx)[4]
    for i in range(shnum):
        sh_name, _, _, _, offset, size = header(i)
        start = names_offset + sh_name
        if data[start:data.index(b"\0", start)].decode() == name:
            return data[offset:offset + size]
    return None


def pe_section(data: bytes, name: str):
    """The bytes of section `name` of a PE file (`.exe`), or None."""
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("not a PE file")
    sections, _, _, _, optional_size = struct.unpack_from("<HIIIH", data, pe + 6)
    table = pe + 24 + optional_size
    for i in range(sections):
        raw_name, virtual_size, _, raw_size, raw_offset = struct.unpack_from("<8sIIII", data, table + i * 40)
        if raw_name.rstrip(b"\0").decode(errors="replace") == name:
            return data[raw_offset:raw_offset + min(virtual_size or raw_size, raw_size)]
    return None


def dependency_list(data: bytes):
    """The crates listed in the binary, or None when it has no `.dep-v0` section."""
    if data[:4] == b"\x7fELF":
        section = elf_section(data, SECTION)
    elif data[:2] == b"MZ":
        section = pe_section(data, SECTION)
    else:
        raise ValueError("neither an ELF nor a PE file")
    if section is None:
        return None
    return json.loads(zlib.decompress(section))["packages"]


def main(paths):
    if not paths:
        print(__doc__.strip().splitlines()[-1])
        return 2
    failed = False
    for path in paths:
        try:
            with open(path, "rb") as f:
                packages = dependency_list(f.read())
        except (OSError, ValueError, struct.error) as e:
            print(f"{path}: cannot be read as a binary – {e}")
            failed = True
            continue
        if packages is None:
            print(f"{path}: no dependency list – built without cargo auditable")
            failed = True
        else:
            names = {p["name"] for p in packages}
            print(f"{path}: {len(packages)} crates listed (e.g. {', '.join(sorted(names)[:3])}, …)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
