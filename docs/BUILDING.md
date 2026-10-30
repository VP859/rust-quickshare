# Building Ferry

## Requirements

* Rust **1.91** or newer (`rustup` reads `rust-toolchain.toml` and installs stable with
  `rustfmt` and `clippy`).
* For the GUI on Linux – WebKitGTK and friends:
  * Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`
  * Fedora: `sudo dnf install webkit2gtk4.1-devel libxdo-devel openssl-devel libappindicator-gtk3-devel librsvg2-devel`
* For the GUI on Windows – WebView2 (preinstalled on Windows 10/11) and the MSVC build tools.
* For Android – JDK 17+, Android SDK (`ANDROID_HOME`), NDK r27 (`NDK_HOME`) and the Rust targets
  `aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android`.
* Node.js – only to run the Tauri CLI through `npx` and the frontend check.

## Library and CLI

```bash
cargo build --release -p ferry                # target/release/ferry
cargo test --workspace                        # unit and end-to-end tests (two real iroh nodes)
cargo bench --bench transfer_benchmarks       # hashing, framing, a 64 MiB transfer over QUIC
cargo doc --no-deps --workspace --open        # API documentation
```

Two independent devices on one computer:

```bash
FERRY_CONFIG_DIR=/tmp/ferry-a cargo run --bin ferry -- tui
FERRY_CONFIG_DIR=/tmp/ferry-b cargo run --bin ferry -- tui
```

## GUI (desktop)

```bash
cd src-tauri
npx @tauri-apps/cli@2 dev                      # hot reload of ui/index.html
npx @tauri-apps/cli@2 build                    # .deb/.rpm/.AppImage or .msi/.exe
```

The `.deb` and `.rpm` packages also install the `ferry` CLI (`beforeBundleCommand` in
`tauri.conf.json`).

## Android

```bash
cargo install tauri-cli --version "^2" --locked   # gen/android/buildSrc calls `cargo tauri`
cd src-tauri
cargo tauri android build --apk --aab
```

Release signing reads `src-tauri/gen/android/keystore.properties` (not committed):

```properties
storeFile=/absolute/path/release.jks
storePassword=…
keyAlias=…
keyPassword=…
```

Install on a connected phone with `adb install -r <apk>`. An installation of the old package
`com.rqs.quickshare` is a different app; remove it with `adb uninstall com.rqs.quickshare`.

## Checks before a pull request

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --workspace
node scripts/check-frontend.mjs src-tauri/ui/index.html
cargo deny check                               # licenses and advisories (cargo install cargo-deny)
```

Android Rust code is checked with clippy for `aarch64-linux-android` in CI; locally:

```bash
BIN=$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin
CC_aarch64_linux_android=$BIN/aarch64-linux-android24-clang \
AR_aarch64_linux_android=$BIN/llvm-ar \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$BIN/aarch64-linux-android24-clang \
cargo clippy -p ferry -p ferry-gui --target aarch64-linux-android -- -D warnings
```

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`: CLI archives and installers
(cargo-dist), GUI installers (Tauri) and the Android APK/AAB are collected into a **draft**
GitHub release with `SHA256SUMS.txt`, an SPDX SBOM and – in public repositories – signed build
provenance attestations. Verify a download with:

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
gh attestation verify <file> --repo VP859/rust-quickshare
```

Android signing in CI uses the `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`,
`ANDROID_KEY_ALIAS` and `ANDROID_KEY_PASSWORD` secrets.
