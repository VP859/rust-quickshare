use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info};

pub const MMAP_THRESHOLD_BYTES: u64 = 1_073_741_824;

pub const MIN_CHUNK_SIZE: usize = 64 * 1024;
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
pub const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;

pub fn compute_hash_mmap_or_stream(path: &Path, file_size: u64) -> Result<blake3::Hash> {
    if file_size >= MMAP_THRESHOLD_BYTES {
        info!(
            "Plik ma rozmiar {:.2} GB >= 1 GB. Aktywowanie odczytu Zero-Copy przez memmap2.",
            file_size as f64 / (1024.0 * 1024.0 * 1024.0)
        );
        compute_mmap_hash(path)
    } else {
        compute_streaming_hash(path)
    }
}

fn compute_mmap_hash(path: &Path) -> Result<blake3::Hash> {
    let file =
        File::open(path).with_context(|| format!("Nie udało się otworzyć pliku: {:?}", path))?;

    let mmap = unsafe {
        memmap2::MmapOptions::new()
            .map(&file)
            .with_context(|| format!("Błąd mapowania pliku do pamięci: {:?}", path))?
    };

    let mut hasher = blake3::Hasher::new();
    hasher.update_rayon(&mmap);
    Ok(hasher.finalize())
}

fn compute_streaming_hash(path: &Path) -> Result<blake3::Hash> {
    let mut file =
        File::open(path).with_context(|| format!("Nie udało się otworzyć pliku: {:?}", path))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; DEFAULT_CHUNK_SIZE];

    use std::io::Read;
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hasher.finalize())
}

#[derive(Debug, Clone)]
pub struct BufferTuner {
    current_chunk_size: usize,
}

impl BufferTuner {
    pub fn new() -> Self {
        Self {
            current_chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }

    pub fn current_chunk_size(&self) -> usize {
        self.current_chunk_size
    }

    pub fn update_metrics(&mut self, rtt: Duration, throughput_bytes_per_sec: u64) {
        let rtt_ms = rtt.as_millis() as u64;

        let bdp = (throughput_bytes_per_sec * rtt_ms) / 1000;

        let new_size = if rtt_ms > 150 {
            (bdp as usize).clamp(DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE)
        } else if throughput_bytes_per_sec < 1_000_000 {
            MIN_CHUNK_SIZE
        } else {
            (bdp as usize).clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE)
        };

        debug!(
            "BufferTuner: RTT={}ms, Thruput={:.2} MB/s -> Nowy rozmiar chunka: {} KB",
            rtt_ms,
            throughput_bytes_per_sec as f64 / 1_048_576.0,
            new_size / 1024
        );

        self.current_chunk_size = new_size;
    }
}

impl Default for BufferTuner {
    fn default() -> Self {
        Self::new()
    }
}
