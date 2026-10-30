//! Criterion benchmarks: hashing, protocol framing and a complete file transfer
//! between two nodes over QUIC on the loopback interface.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ferry::node::{NodeEngine, load_or_generate_secret_key};
use ferry::protocol::codec::{read_msg, write_msg};
use ferry::protocol::messages::{ChunkHeader, DEFAULT_CHUNK_SIZE, TransferOffer};
use ferry::transfer::service::{self, OfferDecision, TransferEvent, TransferHandler};
use iroh::EndpointId;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tokio::runtime::Runtime;

/// BLAKE3 throughput on in-memory buffers of 64 KiB, 1 MiB and 10 MiB.
fn bench_blake3_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake3_hashing");

    for size in [64 * 1024, 1024 * 1024, 10 * 1024 * 1024].iter() {
        let data = vec![0xA5u8; *size];
        group.throughput(Throughput::Bytes(*size as u64));

        group.bench_with_input(
            BenchmarkId::new("hash_buffer", format!("{}KB", size / 1024)),
            size,
            |b, _| {
                b.iter(|| {
                    black_box(blake3::hash(&data));
                });
            },
        );
    }
    group.finish();
}

/// Encoding and decoding of a chunk header and an offer through an in-memory pipe.
fn bench_protocol_codec(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("protocol_codec");

    let header = ChunkHeader {
        chunk_index: 1024,
        payload_len: DEFAULT_CHUNK_SIZE,
        chunk_hash: [0x7F; 32],
    };

    group.bench_function("encode_decode_chunk_header", |b| {
        b.to_async(&rt).iter(|| async {
            let (mut client, mut server) = duplex(1024);

            write_msg(&mut client, &header).await.unwrap();
            client.flush().await.unwrap();

            let decoded: ChunkHeader = read_msg(&mut server).await.unwrap();
            black_box(decoded);
        });
    });

    let offer = TransferOffer {
        file_name: "test_large_archive_distribution.tar.gz".into(),
        file_size: 10_000_000_000,
        chunk_size: DEFAULT_CHUNK_SIZE,
        file_hash: [0xAB; 32],
        folder: Vec::new(),
    };

    group.bench_function("encode_decode_transfer_offer", |b| {
        b.to_async(&rt).iter(|| async {
            let (mut client, mut server) = duplex(2048);

            write_msg(&mut client, &offer).await.unwrap();
            client.flush().await.unwrap();

            let decoded: TransferOffer = read_msg(&mut server).await.unwrap();
            black_box(decoded);
        });
    });

    group.finish();
}

/// Framing overhead of streaming 1 MiB chunks with headers through an in-memory pipe.
fn bench_duplex_stream_transfer(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("in_memory_p2p_pipeline");

    for size_mb in [1, 10, 50].iter() {
        let total_bytes = size_mb * 1024 * 1024;
        let chunk_payload = vec![0x35u8; DEFAULT_CHUNK_SIZE as usize];

        group.throughput(Throughput::Bytes(total_bytes as u64));

        group.bench_with_input(
            BenchmarkId::new("stream_data", format!("{}MB", size_mb)),
            size_mb,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let (mut sender_stream, mut receiver_stream) = duplex(2 * 1024 * 1024);
                    let total_chunks = total_bytes / DEFAULT_CHUNK_SIZE;
                    let payload_clone = chunk_payload.clone();

                    let sender_task = tokio::spawn(async move {
                        for idx in 0..total_chunks {
                            let header = ChunkHeader {
                                chunk_index: idx as u64,
                                payload_len: DEFAULT_CHUNK_SIZE,
                                chunk_hash: [0x11; 32],
                            };
                            write_msg(&mut sender_stream, &header).await.unwrap();
                            sender_stream.write_all(&payload_clone).await.unwrap();
                        }
                        sender_stream.flush().await.unwrap();
                    });

                    let receiver_task = tokio::spawn(async move {
                        let mut buf = vec![0u8; DEFAULT_CHUNK_SIZE as usize];
                        for _ in 0..total_chunks {
                            let _header: ChunkHeader =
                                read_msg(&mut receiver_stream).await.unwrap();
                            receiver_stream.read_exact(&mut buf).await.unwrap();
                            black_box(&buf);
                        }
                    });

                    let _ = tokio::try_join!(sender_task, receiver_task).unwrap();
                });
            },
        );
    }
    group.finish();
}

/// Receiver accepting every offer into one folder.
struct AcceptAll(PathBuf);

impl TransferHandler for AcceptAll {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Accept {
            dest_dir: self.0.clone(),
        }
    }

    fn on_event(&self, _: TransferEvent) {}
}

/// Whole user-visible transfer: hashing on the sender, offer, chunks over QUIC,
/// writing, verification and renaming on the receiver.
fn bench_end_to_end_transfer(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024 * 1024;
    let rt = Runtime::new().unwrap();
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let path = src.path().join("bench.bin");
    let data: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &data).unwrap();

    let (sender, addr) = rt.block_on(async {
        let receiver = NodeEngine::new(load_or_generate_secret_key(None).unwrap())
            .await
            .unwrap();
        let addr = receiver.node_addr().await.unwrap();
        tokio::spawn(service::serve(
            receiver,
            Arc::new(AcceptAll(dst.path().to_path_buf())),
        ));
        let sender = NodeEngine::new(load_or_generate_secret_key(None).unwrap())
            .await
            .unwrap();
        (sender, addr)
    });

    let mut group = c.benchmark_group("end_to_end_transfer");
    group.throughput(Throughput::Bytes(SIZE as u64));
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));
    group.bench_function("push_64MiB_loopback", |b| {
        b.to_async(&rt).iter(|| async {
            service::send_files(&sender, addr.clone(), std::slice::from_ref(&path), &|_| {})
                .await
                .unwrap();
            std::fs::remove_file(dst.path().join("bench.bin")).unwrap();
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_blake3_hashing,
    bench_protocol_codec,
    bench_duplex_stream_transfer,
    bench_end_to_end_transfer
);
criterion_main!(benches);
