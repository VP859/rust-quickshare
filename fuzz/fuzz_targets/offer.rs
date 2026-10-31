//! Structured file offers: a valid offer must describe chunks that cover the
//! file exactly and only safe names.
#![no_main]

use arbitrary::Arbitrary;
use ferry::protocol::messages::TransferOffer;
use ferry::transfer::security::sanitize_filename;
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
struct Input {
    file_name: String,
    file_size: u64,
    chunk_size: u32,
    folder: Vec<String>,
}

fuzz_target!(|input: Input| {
    let offer = TransferOffer {
        file_name: input.file_name,
        file_size: input.file_size,
        chunk_size: input.chunk_size,
        file_hash: [0; 32],
        folder: input.folder,
    };
    if offer.validate().is_err() {
        return;
    }
    let chunks = offer.total_chunks();
    // The last chunk ends exactly at the end of the file.
    if chunks > 0 {
        let last = offer.chunk_len(chunks - 1).expect("last chunk exists");
        assert!(last > 0 && last <= offer.chunk_size);
        assert_eq!((chunks - 1) * u64::from(offer.chunk_size) + u64::from(last), offer.file_size);
    }
    assert_eq!(offer.chunk_len(chunks), None);
    for part in offer.folder.iter().chain(std::iter::once(&offer.file_name)) {
        if let Ok(name) = sanitize_filename(part) {
            assert!(!name.contains(['/', '\\']) && name != ".." && name != ".");
        }
    }
});
