# Ferry compared with other tools

There are many good ways to move a file from one device to another. This page says where Ferry
fits, and just as plainly where another tool will serve you better. It describes the other projects
as we understand them (2026). They change often, so check their own documentation before deciding,
and [open an issue](https://github.com/VP859/rust-quickshare/issues) if something here is wrong or
out of date.

## Overview

| | Ferry | [LocalSend](https://localsend.org) | [KDE Connect](https://kdeconnect.kde.org) | [Warpinator](https://github.com/linuxmint/warpinator) | [PairDrop](https://pairdrop.net) | [croc](https://github.com/schollz/croc) / [magic-wormhole](https://magic-wormhole.readthedocs.io) | Quick Share / AirDrop |
|---|---|---|---|---|---|---|---|
| Main purpose | file and text transfer | file and text transfer | phone–computer integration | file transfer | file transfer in the browser | file transfer from the terminal | file transfer built into the system |
| Platforms | Linux, Windows, Android | all, including iOS and macOS | Linux, Windows, macOS, Android, iOS (limited) | Linux (Windows and Android ports) | any browser | any with a terminal | Android and Windows / Apple devices |
| Needs a shared network | yes, or optional relays | yes | yes | yes | no (a server connects the browsers) | no (a relay connects the devices) | no (Bluetooth and Wi-Fi Direct) |
| Trust | pairing with a code, then trusted devices | none: anyone on the network can offer files | pairing | group code | room or pairing | one-time code phrase | accounts and contacts |
| Interfaces | GUI, terminal UI, CLI, daemon | GUI, CLI | GUI and many integrations | GUI | web page | CLI | system share menu |
| Open source | yes | yes | yes | yes | yes | yes | no |
| Released, in app stores | not yet | yes | yes | yes (Linux Mint) | yes (web) | yes | built in |

## Where Ferry is worse

- **No app for iPhone, iPad or Mac.** LocalSend and KDE Connect have one, AirDrop is built in. An
  Apple device can send to Ferry through the LocalSend app or a browser session, but Ferry can
  send to it only through a browser session.
- **Not released yet, not in any app store.** There is no Flathub, F-Droid, winget or Play Store
  package (see the [roadmap](../ROADMAP.md)). LocalSend, KDE Connect and Warpinator are a click away
  in most stores and distributions.
- **Young and not audited.** Ferry has many tests, fuzzing and a written
  [security model](SECURITY-MODEL.md), but no independent review, few users and no track record.
  Most of its code was written with an AI assistant ([details](../GOVERNANCE.md#ai-assisted-development)).
  magic-wormhole and KDE Connect have been used and examined by many people for years.
- **Needs a shared network, or the internet.** Quick Share and AirDrop also work between two
  phones with no Wi-Fi at all, over Bluetooth and Wi-Fi Direct. Ferry needs both devices on the
  same network, or the internet mode and its relays.
- **Only Ferry speaks Ferry's protocol.** Apart from receiving from LocalSend apps and browsers,
  both sides need Ferry. A LocalSend user can talk to every LocalSend app on every platform.
- **Does one thing.** KDE Connect also syncs the clipboard, shows phone notifications on the
  computer, controls media and works as a remote mouse. Ferry only moves files and text.
- **The other side must have Ferry installed** for its full security. PairDrop works with nothing
  but a browser on both sides; Ferry's browser sessions need Ferry on one side and use plain HTTP.
- **Two languages.** Ferry speaks English and Polish; LocalSend and KDE Connect are translated into
  dozens of languages.
- **No anonymity.** Who talks to whom, when and how much is visible on the local network and, in
  the internet mode, to the relay operator. [OnionShare](https://onionshare.org) sends over Tor and
  hides this, at the cost of speed.
- **Android in the background depends on the phone.** Battery savers can pause Ferry while it waits
  for files; the system apps (Quick Share) do not have this problem.

## Where Ferry is better

- **Trust you can check.** Devices pair once with a six-digit code shown on both screens; after
  that, a trusted device is recognised by its key, not its name; announcements on the network are
  signed, and a safety number can be compared at any time (`ferry peer verify`). LocalSend accepts offers
  from anyone on the network and cannot tell two devices with the same name apart.
- **Private by default.** No telemetry, no account, no server in the default mode; a hidden mode
  shows the device only to paired devices; files shared by TICKET use a one-time identity.
- **Across networks without a third-party server**, if you want: the internet mode can use your
  own relay instead of public ones. croc and magic-wormhole also cross networks, but through their
  relays (which can be self-hosted as well).
- **Large files and bad networks.** Every chunk is verified, interrupted transfers resume where
  they stopped, a broken connection is retried on its own, and a transfer can be paused or limited
  in speed ([benchmarks](BENCHMARKS.md), [network tests](TESTING.md#bad-network-conditions)).
- **Every interface.** The same device can be used from the desktop app, the terminal UI, the
  command line and a background daemon, which is useful on servers and in scripts.

## Choosing

- Sending between your own Linux, Windows and Android devices, with trust that matters to you:
  **Ferry**.
- A household with iPhones and Macs, or the widest choice of platforms: **LocalSend**.
- Using your phone from the computer (notifications, clipboard, media): **KDE Connect**.
- Two phones with no network at all: **Quick Share** or **AirDrop**.
- A one-off transfer to someone on another network, from the terminal: **croc** or
  **magic-wormhole**.
- Nothing installed on either side: **PairDrop**.
- Hiding who sends to whom: **OnionShare**.
