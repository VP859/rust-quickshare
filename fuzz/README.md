# Fuzzing

Everything Ferry parses from other devices or from pasted text has a fuzz target:

| Target | Input |
|--------|-------|
| `frame` | bytes on a control stream, decoded as every protocol message, plus validation |
| `offer` | structured file offers: chunk arithmetic and names |
| `ticket` | TICKET text: parsing and round trip |
| `file_name` | file and device names: sanitization invariants |
| `announcement` | UDP discovery datagrams and announcement bodies |

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
cargo +nightly fuzz run frame -- -max_total_time=300   # or any target above
cargo +nightly fuzz list
```

A crash is saved in `fuzz/artifacts/<target>/`; reproduce it with
`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>` and add the input as a regular
test once fixed. The corpus in `fuzz/corpus/` is not committed. CI runs every target for a few
minutes each week (`.github/workflows/fuzz.yml`).
