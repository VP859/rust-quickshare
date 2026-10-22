use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};

/// Domyślny rozmiar chunka (1 MiB).
pub const DEFAULT_CHUNK_SIZE: u32 = 1024 * 1024;
/// Górny limit rozmiaru chunka akceptowany od zdalnego węzła.
pub const MAX_CHUNK_SIZE: u32 = 16 * 1024 * 1024;
/// Maksymalny rozmiar ramki metadanych (oferta, nagłówek chunka itd.).
pub const MAX_FRAME_LEN: u32 = 64 * 1024;
/// Maksymalna długość nazwy pliku w bajtach (limit większości systemów plików).
pub const MAX_FILE_NAME_LEN: usize = 255;
/// Maksymalna długość nazwy urządzenia (parowanie, wykrywanie w sieci).
pub const MAX_DEVICE_NAME_LEN: usize = 64;
/// Liczba strumieni chunków przesyłanych równolegle.
pub const CHUNK_WINDOW: usize = 8;

/// Token udostępnienia pliku przez TICKET (tryb "pull").
pub type ShareToken = [u8; 16];

/// Pierwsza ramka na strumieniu kontrolnym otwieranym przez stronę wybierającą połączenie.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Hello {
    /// Strona łącząca się wysyła plik (wysyłka do zaufanego urządzenia / demona).
    Push,
    /// Strona łącząca się pobiera plik udostępniony pod tokenem z TICKETU.
    Pull { token: ShareToken },
    /// Prośba o wzajemne sparowanie (dodanie do zaufanych urządzeń).
    /// `addr` – aktualny adres proszącego, zapamiętywany przez drugą stronę.
    Pair { name: String, addr: EndpointAddr },
}

/// Odpowiedź na [`Hello::Pair`] po decyzji użytkownika.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResponse {
    pub accepted: bool,
    pub name: String,
    pub addr: Option<EndpointAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOffer {
    pub file_name: String,
    pub file_size: u64,
    pub chunk_size: u32,
    pub file_hash: [u8; 32],
}

impl TransferOffer {
    /// Odrzuca oferty, które mogłyby doprowadzić do paniki lub nadmiernej alokacji.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.chunk_size == 0 || self.chunk_size > MAX_CHUNK_SIZE {
            anyhow::bail!("Nieprawidłowy rozmiar chunka: {}", self.chunk_size);
        }
        if self.file_name.is_empty() || self.file_name.len() > MAX_FILE_NAME_LEN {
            anyhow::bail!("Nieprawidłowa długość nazwy pliku");
        }
        Ok(())
    }

    pub fn total_chunks(&self) -> u64 {
        self.file_size.div_ceil(self.chunk_size as u64)
    }

    /// Oczekiwana długość chunka o danym indeksie (ostatni może być krótszy).
    pub fn chunk_len(&self, index: u64) -> Option<u32> {
        let start = index.checked_mul(self.chunk_size as u64)?;
        if start >= self.file_size {
            return None;
        }
        Some((self.file_size - start).min(self.chunk_size as u64) as u32)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferDecision {
    Accept { start_chunk_index: u64 },
    Reject { reason: String },
}

/// Nagłówek poprzedzający dane na każdym jednokierunkowym strumieniu chunka.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkHeader {
    pub chunk_index: u64,
    pub payload_len: u32,
    pub chunk_hash: [u8; 32],
}

/// Końcowy werdykt odbiorcy po weryfikacji haszu całego pliku.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferResult {
    Verified,
    Failed { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(size: u64, chunk: u32) -> TransferOffer {
        TransferOffer {
            file_name: "a.bin".into(),
            file_size: size,
            chunk_size: chunk,
            file_hash: [0; 32],
        }
    }

    #[test]
    fn chunk_math() {
        let o = offer(2 * 1024 * 1024 + 5, DEFAULT_CHUNK_SIZE);
        assert_eq!(o.total_chunks(), 3);
        assert_eq!(o.chunk_len(0), Some(DEFAULT_CHUNK_SIZE));
        assert_eq!(o.chunk_len(2), Some(5));
        assert_eq!(o.chunk_len(3), None);
        assert_eq!(offer(0, DEFAULT_CHUNK_SIZE).total_chunks(), 0);
    }

    #[test]
    fn rejects_malicious_offers() {
        assert!(offer(10, 0).validate().is_err());
        assert!(offer(10, MAX_CHUNK_SIZE + 1).validate().is_err());
        let mut o = offer(10, DEFAULT_CHUNK_SIZE);
        o.file_name = "x".repeat(300);
        assert!(o.validate().is_err());
        assert!(offer(10, DEFAULT_CHUNK_SIZE).validate().is_ok());
    }
}
