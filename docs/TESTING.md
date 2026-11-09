# Testing Ferry

| Kind | Where | Command | In CI |
|------|-------|---------|-------|
| Unit tests | next to the code (`#[cfg(test)]`) | `cargo test --workspace` | every push |
| End-to-end tests (two real iroh nodes) | `tests/integration_tests.rs` | `cargo test --test integration_tests` | every push |
| Property-based tests | `tests/properties.rs`, `transfer/service.rs` | `cargo test --test properties` (more cases: `PROPTEST_CASES=20000`) | every push |
| Bad network conditions | `tests/network_conditions.rs` | `cargo test --release --test network_conditions -- --include-ignored` | every push (job *Bad network conditions*) |
| The same end-to-end tests over a lossy kernel link | `tc netem` on loopback | see below | every push |
| Sharing by TICKET (one-time identity) | `tests/share.rs` | `cargo test --test share` | every push |
| Protocol versions | `tests/version_negotiation.rs` | `cargo test --test version_negotiation` | every push |
| Protocol vectors | `tests/protocol_vectors.rs`, `scripts/verify-vectors.py` (independent, from the spec) | `cargo test --test protocol_vectors`; `python3 scripts/verify-vectors.py` | every push |
| Pause, speed limit, history, several devices | `tests/control.rs` (real nodes; the limit is global, so its own process) | `cargo test --test control` | every push |
| "Send with Ferry" in file managers | `src-tauri/src/open_files.rs` (command line → files), `cli/src/cli/integrate_cmd.rs`, `tests/integration/test_nautilus_extension.py` (stand-ins for Nautilus), GUI test of the handover | `cargo test -p ferry-gui -p ferry-cli`; `python3 -I tests/integration/test_nautilus_extension.py` | every push |
| Device key in the system keyring | unit tests with an in-memory store (`node::identity`, `node::keystore`); `tests/keystore.rs` against the real Secret Service / Credential Manager | `cargo test --test keystore -- --include-ignored` (needs a desktop session) | unit tests on every push; the real store by hand |
| Own relay mode | `tests/relay.rs` (a real iroh relay server in the test; two devices that know only its URL transfer a file) | `cargo test --test relay` | every push |
| Browser sessions | `tests/web.rs` (token, first device owns it, expiry, downloads, uploads, names and sizes; everything encrypted, and refused with another key or for another file); `src/web/seal.rs` (the encryption: every size, changed, cut, reordered and foreign data); `tests/gui/web-page.spec.js` (the page in Chromium against an encrypting mock, and a test vector shared with the Rust side); `npm run check:ui` checks `src/web/page/vendor/` against the npm package | `cargo test --test web`; `npm run test:ui` | every push |
| LocalSend compatibility mode | `tests/localsend.rs` (client written from the LocalSend spec, pins the certificate like the official one) | `cargo test --test localsend` (also multicast and the Python client: `-- --include-ignored`) | every push; the Python client on Linux |
| LocalSend, independent client | `scripts/localsend-client.py` (Python standard library only) | see [below](#localsend-without-apple-devices) | every push (through `tests/localsend.rs`) |
| Fuzzing | `fuzz/` | `cargo +nightly fuzz run frame` ([fuzz/README.md](../fuzz/README.md)) | 1 min per target on pull requests that touch `src/`, 15 min weekly |
| Coverage | `cargo llvm-cov` | `cargo llvm-cov -p ferry --fail-under-lines 78` | every push |
| Benchmarks | `benches/transfer_benchmarks.rs` | `cargo bench --bench transfer_benchmarks` | pull requests, compared with the base branch |
| GUI frontend | `tests/gui/` (Playwright, mocked Tauri API) | `npm ci && npm run test:ui` | every push |
| Accessibility | `tests/gui/accessibility.spec.js`: [axe-core](https://github.com/dequelabs/axe-core) (the WCAG 2.0–2.2 A and AA rules) and Ferry's own audit in `tests/gui/audit.js`: names of controls, labels, dialogs, unique IDs and WCAG AA contrast (from the rendered colours) of the main screen, every dialog and the browser session page, in the light and the dark theme; keyboard (focus kept in dialogs, Escape, visible focus, tab pattern), reduced motion | `npx playwright test tests/gui/accessibility.spec.js` | every push (with the GUI frontend) |
| Android app (Kotlin, Gradle, Rust for arm64) | `src-tauri/gen/android/` | `cd src-tauri && cargo tauri android build --debug --apk --target aarch64` | every push; the debug APK is kept for 7 days as an artifact |

## LocalSend without Apple devices

The LocalSend app is one Flutter code base for iOS, macOS, Android, Windows and Linux, and its
network protocol is one Rust crate (`packages/core` in github.com/localsend/localsend). An iPhone
therefore talks to Ferry exactly like LocalSend on an Android phone or a Linux PC; nothing in the
protocol depends on the platform (`deviceType` only chooses an icon). What is tested, from the
cheapest to the most realistic:

1. **`tests/localsend.rs`** – a client written from the specification that behaves like the
   official one (HTTPS, certificate pinned to the announced fingerprint, `register`,
   `prepare-upload`, one `upload` per file, `cancel`): files and folders, declining and the 15 s
   wait, tokens, checksums, wrong sizes, path traversal, risky files, cancelling, a stable
   certificate, and with `--include-ignored` the multicast exchange with a fake LocalSend app on
   the same computer.
2. **`scripts/localsend-client.py`** – an independent client in Python (no Ferry code), run by
   the same test file in CI. It also works by hand from any other computer on the network:

   ```bash
   python3 scripts/localsend-client.py discover            # Ferry must appear in the list
   python3 scripts/localsend-client.py send 192.168.1.20 photo.jpg Folder/
   ```

3. **The real LocalSend app on an Android phone** (Google Play or F-Droid) or on another
   Windows/Linux computer: with `localsend true` in Ferry, open LocalSend → *Send*; Ferry must be
   listed under its device name; send a photo, a folder and a text message. On the same computer
   the two cannot run at once (both need port 53317).
4. **The official command-line client** `localsend-cli` (`cli/` in the LocalSend repository, the
   same Rust core as the apps): `localsend-cli --port 53318 send --to <Ferry's IP> file.jpg`.

Checklist for the real app: Ferry appears within a few seconds of opening LocalSend; the offer
dialog says *LocalSend · not verified* with the phone's IP address; declining shows *Declined* on
the phone; files keep their folder structure; a second phone sending at the same time is told the
receiver is busy; cancelling on the phone stops the transfer in Ferry.

## Bad network conditions

`tests/network_conditions.rs` puts a UDP forwarder (`LossyLink`) between two nodes and checks that
transfers arrive intact:

- with 3 % packet loss and 40 ms round trip,
- when the sender's address changes in the middle (a switch from Wi-Fi to mobile data),
- after a 5 s outage (Wi-Fi drops, the phone locks),
- after an outage longer than the QUIC idle timeout (a phone in deep sleep): the connection breaks,
  the sender reconnects by itself, the receiver resumes the partial file and does not ask the
  user again. This test takes about 40 s and is `#[ignore]`d locally.

The outage and network-change tests are skipped on Windows. On GitHub's Windows runners both stopped
at the 120 s limit, while they take about 10 s on Linux. The cause is not known yet. Ruled out so far:

- **ICMP "port unreachable" errors on UDP sockets** (Windows reports them on the next receive): iroh's
  QUIC sockets switch them off (`SIO_UDP_CONNRESET`), and the test forwarder skips them.
- **Timer resolution** (15.6 ms on Windows): the tests pass on Linux with every forwarded packet
  delayed to a 15.6 ms tick.
- **Small UDP receive buffers** (64 KiB by default on Windows): the tests pass on Linux with 8 KiB
  forwarder buffers (15 s instead of 10 s).

To find it, the CI step *Network-condition tests on Windows* runs them on Windows without failing
the build. On a timeout a test prints its progress every 2 s: bytes sent, packets forwarded in each
direction, dropped packets, ICMP resets and I/O errors. It also prints the QUIC logs (`RUST_LOG`). A
timeline that stops growing points to a stalled connection, and one that grows slowly points to
throughput. The same run locally: `RUST_LOG=info,noq_proto::connection=debug cargo test --test
network_conditions -- --include-ignored transfer_survives`.

No root rights are needed. To repeat the end-to-end tests with the kernel losing packets (as CI
does):

```bash
sudo tc qdisc add dev lo root netem delay 5ms 2ms loss 1%
cargo test --release --test integration_tests
sudo tc qdisc del dev lo root
```

## Benchmarks

```bash
cargo bench --bench transfer_benchmarks                       # all
cargo bench --bench transfer_benchmarks -- end_to_end         # one group
FERRY_BENCH_MIB=3072 cargo bench --bench transfer_benchmarks -- end_to_end   # a 3 GiB file
```

Comparing two revisions the way CI does:

```bash
git switch main && cargo bench --bench transfer_benchmarks -- --save-baseline base
git switch my-branch && cargo bench --bench transfer_benchmarks -- --baseline base
node scripts/bench-compare.mjs "${CARGO_TARGET_DIR:-target}/criterion" 15
```

A benchmark fails the comparison only when even the lower bound of its 95 % confidence interval is
more than 15 % slower, because shared CI machines are noisy.

## Android, by hand

The background behaviour depends on the phone and cannot run in CI. With the phone connected
(`adb devices`) and Ferry open:

1. **Deep sleep (Doze)** during a transfer of a large file to the phone:
   ```bash
   adb shell dumpsys deviceidle force-idle      # the phone behaves as after a long sleep
   # wait 1–2 minutes, then:
   adb shell dumpsys deviceidle unforce
   ```
   Expected: the transfer resumes on its own ("Connection lost – reconnecting") and completes
   without a second question. With battery optimization off (Settings → Background) it should not
   even stop.
2. **App standby / restricted bucket:**
   ```bash
   adb shell am set-standby-bucket io.github.vp859.ferry restricted
   adb shell am get-standby-bucket io.github.vp859.ferry
   ```
   Expected: *Settings → Background* in Ferry shows the restriction. Restore with `active`.
3. **Battery saver:** `adb shell settings put global low_power 1` (back: `0`); the settings show it.
4. **Foreground service type:** `adb shell dumpsys activity services io.github.vp859.ferry` shows
   the service and its type (`connectedDevice`, or `dataSync` if the phone refused the first). The
   6-hour daily limit of Android 15+ applies only to `dataSync`; when it is reached Ferry stops the
   service and shows a notification that opens the app.
5. **Screen off during a transfer:** the notification shows "Transferring files…" and the speed
   stays the same as with the screen on (wake and Wi-Fi locks).

Logs: `adb logcat -s FerryCore FerryService FerryShare FerryPower`.
