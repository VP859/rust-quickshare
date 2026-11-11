# Roadmap

What is planned for Ferry, roughly in order. There are no dates: Ferry is made in the maintainer's
free time ([GOVERNANCE.md](GOVERNANCE.md)). What is already done is in the [changelog](CHANGELOG.md).
To propose something, [open an issue](https://github.com/VP859/rust-quickshare/issues); to work on
an item, say so in its issue first.

## Before the first release

- [ ] **Reproducible builds** with instructions for checking that a published file was built from
  the tagged source.
- [ ] **Signed releases**: Windows installers signed through [SignPath Foundation](https://signpath.org)
  (free for open-source projects), `minisign` signatures for the Linux packages and the checksums,
  and a published signing key.
- [ ] **A first run of the release workflow** and a test of every package on a clean system.
- [ ] **Supply-chain checks**: the OpenSSF Best Practices badge, publishing the
  [OpenSSF Scorecard](https://scorecard.dev) results once the repository is public (it already runs
  in CI), later `cargo vet`. Done: `cargo auditable` for the released binaries (the dependency list
  built into them).

## Soon after

- [ ] **Distribution channels**: Flathub, F-Droid, winget and the AUR. F-Droid needs the reproducible
  builds above.
- [ ] **Independent security audit**: an application to [NLnet NGI Zero](https://nlnet.nl), which
  funds open-source projects and arranges free security and accessibility reviews, or to
  [OSTIF](https://ostif.org).
- [ ] **Translations on [Weblate](https://weblate.org)** (free for open-source projects), so that
  more languages can be added without changes to the code
  ([TRANSLATING.md](docs/TRANSLATING.md)).
- [ ] **The network-condition tests on Windows**: they stall on GitHub's Windows runners for a reason
  not found yet and are skipped there ([TESTING.md](docs/TESTING.md#bad-network-conditions)).

## Later, maybe

These are ideas, not promises; an issue with a use case makes them more likely.

- Sending texts to LocalSend apps (files and folders already work).
- Benchmarks on real Wi-Fi and mobile networks, published on the [benchmarks](docs/BENCHMARKS.md)
  page.
- More background reliability on Android phones with aggressive battery savers.

## Not planned

- **iOS and macOS apps.** They need Apple hardware and a paid developer account for testing and
  publishing. Apple devices can send to Ferry through LocalSend or a browser session.
- **Accounts, cloud storage or telemetry** of any kind.
- **Features beyond moving files and text** (notifications, remote input, media control); KDE
  Connect does these well ([comparison](docs/COMPARISON.md)).
