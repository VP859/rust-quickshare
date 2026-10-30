# Security policy

Ferry moves files between devices and accepts data from other devices on the network, so
security reports are taken seriously.

## Supported versions

Ferry is in early development (`0.x`). Only the latest release receives security fixes.

| Version | Supported |
|---------|-----------|
| latest `0.x` release | ✅ |
| older releases | ❌ |

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately through GitHub: open the repository's **Security** tab and choose
**Report a vulnerability** (GitHub private vulnerability reporting). If that is not available,
contact the maintainer through the contact listed on <https://github.com/VP859> and ask for a
private channel.

Please include:

- the Ferry version (`ferry --version`) and platform (Linux, Windows or Android version),
- the network mode (`ferry network`) and whether the devices were paired,
- steps to reproduce, and what an attacker gains (e.g. writing files outside the download folder,
  pairing without consent, reading transferred data),
- a proof of concept if you have one.

## What to expect

- Acknowledgement within 7 days.
- An assessment and, for confirmed issues, a fix or mitigation plan within 30 days.
- Credit in the release notes, unless you prefer to stay anonymous.

Please give us a reasonable time to fix the issue before disclosing it publicly.

## Scope

In scope: the `ferry` library, CLI, TUI and daemon, the desktop and Android GUI, the transfer and
pairing protocol, local network discovery, and the release artifacts.

Out of scope: vulnerabilities in third-party services used by the optional internet mode (the public
iroh relays and DNS servers operated by n0), and attacks that require full control of a trusted,
already paired device.

The design, the protections and their known limits are described in
[docs/SECURITY-MODEL.md](docs/SECURITY-MODEL.md).
