use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rqs_p2p::protocol::codec::{read_msg, write_msg};
use rqs_p2p::protocol::messages::{ChunkHeader, TransferOffer, DEFAULT_CHUNK_SIZE};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;
use std::hint::black_box;


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

fn bench_protocol_codec(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("protocol_codec");

    let header = ChunkHeader {
        chunk_index: 1024,
        payload_len: DEFAULT_CHUNK_SIZE as u32,
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
        chunk_size: DEFAULT_CHUNK_SIZE as u32,
        file_hash: [0xAB; 32],
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
                                payload_len: DEFAULT_CHUNK_SIZE as u32,
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
                            let _header: ChunkHeader = read_msg(&mut receiver_stream).await.unwrap();
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

criterion_group!(
    benches,
    bench_blake3_hashing,
    bench_protocol_codec,
    bench_duplex_stream_transfer
);
criterion_main!(benches);