# Contributing to Ferry

Thank you for considering a contribution! Bug reports, translations, documentation and code are
all welcome.

## Ground rules

- Be respectful – this project follows the [Code of Conduct](CODE_OF_CONDUCT.md).
- Security problems are reported privately, see [SECURITY.md](SECURITY.md).
- For larger changes, open an issue first so the approach can be discussed before you spend time
  on it.

## Project layout

| Path | Contents |
|------|----------|
| `src/` | `ferry` library: protocol, transfers, discovery, pairing, configuration |
| `cli/` | `ferry-cli` crate: the `ferry` binary (CLI, terminal UI, daemon, `ferry doctor`) |
| `src-tauri/` | `ferry-gui` crate: Tauri v2 desktop and Android app; `ui/` is the frontend |
| `src-tauri/gen/android/` | Android project (Kotlin: share target, foreground service) |
| `locales/app.yml` | translations of the Rust side (English and Polish) |
| `tests/`, `benches/` | end-to-end tests on two real nodes and Criterion benchmarks |
| `docs/` | architecture, protocol, security model, building and translating |

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the parts fit together.

## Development setup

You need a stable Rust toolchain (see `rust-version` in `Cargo.toml`) and, for the GUI, the
WebKitGTK development packages. The full list per platform is in [docs/BUILDING.md](docs/BUILDING.md).

```bash
cargo build                    # library, CLI and GUI
cargo run --bin ferry -- tui   # terminal UI
cd src-tauri && cargo tauri dev   # GUI with hot reload of ui/
```

Use `FERRY_CONFIG_DIR=/tmp/ferry-a` and `FERRY_CONFIG_DIR=/tmp/ferry-b` to run two independent
devices on one computer.

## Before you open a pull request

All of these must pass – CI runs them too:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --workspace
cargo rustdoc -p ferry-cli --bin ferry -- -D warnings --document-private-items
npm ci && npm run check:ui && npm run lint:ui && npm run test:ui   # when the GUI changed
```

Guidelines:

- **Documentation:** every item, including private ones, has a doc comment (enforced by lints).
  Comments explain *why*, not *what*.
- **Errors and panics:** no `unwrap()`/`expect()` outside tests unless the invariant is documented.
  `unsafe` is denied except in the few modules that need FFI, each block with a `// SAFETY:` comment.
  Library errors are `anyhow` errors with localized context; a failure that callers may want to
  handle differently gets a typed error in `src/error.rs` and an `ErrorKind`.
- **User-facing text** goes through `t!("key")` with entries in `locales/app.yml` for every
  language; the GUI keeps its strings in `src-tauri/ui/js/locales/<code>.js`.
- **Tests:** new behaviour comes with a test; protocol changes come with an end-to-end test in
  `tests/integration_tests.rs`.
- **Protocol changes** follow [docs/PROTOCOL.md](docs/PROTOCOL.md): an incompatible change bumps
  the ALPN.

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(gui): show whether a transfer is direct or relayed
fix(discovery): ignore announcements with an invalid signature
docs: describe the network modes
```

Common types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`.
Breaking changes add `!` (`feat!: …`) and a `BREAKING CHANGE:` footer.

## Translations

See [docs/TRANSLATING.md](docs/TRANSLATING.md) for adding or improving a language.

## License

Ferry is licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in Ferry by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
