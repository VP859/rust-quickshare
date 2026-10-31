# Testing Ferry

| Kind | Where | Command | In CI |
|------|-------|---------|-------|
| Unit tests | next to the code (`#[cfg(test)]`) | `cargo test --workspace` | every push |
| End-to-end tests (two real iroh nodes) | `tests/integration_tests.rs` | `cargo test --test integration_tests` | every push |
| Property-based tests | `tests/properties.rs`, `transfer/service.rs` | `cargo test --test properties` (more cases: `PROPTEST_CASES=20000`) | every push |
| Bad network conditions | `tests/network_conditions.rs` | `cargo test --release --test network_conditions -- --include-ignored` | every push (job *Bad network conditions*) |
| The same end-to-end tests over a lossy kernel link | `tc netem` on loopback | see below | every push |
| Fuzzing | `fuzz/` | `cargo +nightly fuzz run frame` ([fuzz/README.md](../fuzz/README.md)) | 1 min per target on pull requests that touch `src/`, 15 min weekly |
| Coverage | `cargo llvm-cov` | `cargo llvm-cov -p ferry --fail-under-lines 78` | every push |
| Benchmarks | `benches/transfer_benchmarks.rs` | `cargo bench --bench transfer_benchmarks` | pull requests, compared with the base branch |
| GUI frontend | `tests/gui/` (Playwright, mocked Tauri API) | `npm ci && npm run test:ui` | every push |

## Bad network conditions

`tests/network_conditions.rs` puts a UDP forwarder (`LossyLink`) between two nodes and checks that
transfers arrive intact:

- with 3 % packet loss and 40 ms round trip,
- when the sender's address changes in the middle (a switch from Wi-Fi to mobile data),
- after a 5 s outage (Wi-Fi drops, the phone locks),
- after an outage longer than the QUIC idle timeout (a phone in deep sleep): the connection breaks,
  the sender reconnects by itself, the receiver resumes the partial file and does not ask the
  user again. This test takes about 40 s and is `#[ignore]`d locally.

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
