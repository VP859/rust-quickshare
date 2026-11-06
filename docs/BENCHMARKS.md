# Benchmarks

How fast Ferry is, measured so that anyone can repeat it. The numbers below come from one computer
and say more about Ferry's own overhead than about the speed you will see at home: there, the
network (Wi-Fi above all) is almost always the limit.

## What is measured

[`benches/transfer_benchmarks.rs`](../benches/transfer_benchmarks.rs) (Criterion):

| Benchmark | What it shows |
|---|---|
| `blake3_hashing` | hashing speed; every file is hashed once by the sender and once by the receiver |
| `protocol_codec` | encoding and decoding of protocol messages |
| `in_memory_p2p_pipeline` | framing chunks with headers, without network or encryption |
| `end_to_end_transfer` | a whole transfer between two Ferry nodes over QUIC on the loopback interface: hashing, offer, encryption, chunks, writing, verification and renaming |

The end-to-end benchmark is the one that matters: everything a user waits for except the network
itself.

## Results

Measured on 2026-11-06 with Ferry 0.2.9 (release profile), Rust 1.99, Linux 7.2 (Fedora 44), an
Intel Core i7-13700H laptop with 64 GB of memory; the files were in `/tmp` (in memory), so the disk
did not limit the speed. The ranges are Criterion's 95 % confidence intervals.

| Benchmark | Time | Throughput |
|---|---|---|
| End-to-end transfer, 64 MiB file | 157–164 ms | **389–408 MiB/s** |
| End-to-end transfer, 1 GiB file | 1.87–1.97 s | **521–546 MiB/s** |
| BLAKE3, 10 MiB buffer | 2.59 ms | 3.77 GiB/s |
| Framing 50 MB of chunks in memory | 9.7–11.4 ms | 4.3–5.1 GiB/s |
| Encoding and decoding a chunk header | 372 ns | – |
| Encoding and decoding a file offer | 415 ns | – |

Larger files are faster because the chunk size grows with the file (fewer chunks, less overhead
per byte).

What this means:

- On **Wi-Fi** (typically 10–100 MB/s in practice) or **gigabit Ethernet** (about 110 MB/s), the
  network is the limit, not Ferry.
- On faster links (2.5 or 10 Gbit/s Ethernet), Ferry's encryption and hashing on one CPU core start
  to matter; the numbers above are a rough upper bound.
- **Through a relay** (the internet mode when no direct connection is possible), the relay's and the
  internet connection's speed decide.

## What is not measured yet

- **Real networks.** Measurements over Wi-Fi, mobile data and through relays, on several devices,
  are planned ([roadmap](../ROADMAP.md)). Results from your own devices are welcome in an issue:
  say which devices, which network, the file size and the speed Ferry showed.
- **Other tools.** We have not measured LocalSend, croc or others under the same conditions, so we
  do not publish numbers for them. A fair comparison needs the same devices, the same network and
  the same files for every tool; contributions that do this are welcome.
- **Android phones**, whose CPUs, storage and battery savers make them slower than a laptop.

## Repeating the measurements

```bash
cargo bench --bench transfer_benchmarks                                     # all, a few minutes
cargo bench --bench transfer_benchmarks -- end_to_end                       # only the transfer
FERRY_BENCH_MIB=1024 cargo bench --bench transfer_benchmarks -- end_to_end  # a 1 GiB file
```

Criterion's report is in `target/criterion/report/index.html`. Close other programs, connect a
laptop to power and run it twice; the first run also compiles.

## Speed over bad networks

The [network-condition tests](TESTING.md#bad-network-conditions) check that transfers arrive intact
with packet loss and latency, after an outage and after a change of network. They check
correctness, not speed.

## Regressions

CI runs the benchmarks on every pull request against the target branch and fails when a benchmark
gets clearly slower ([TESTING.md](TESTING.md#benchmarks)). Shared CI machines are noisy, so these
runs catch large regressions, not small ones, and their numbers are not comparable with the ones
above.
