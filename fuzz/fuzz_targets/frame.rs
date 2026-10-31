//! Bytes a peer sends on a control stream, decoded as every message type, and
//! the validation of the messages that carry sizes and names.
#![no_main]

use ferry::protocol::codec::decode_frame;
use ferry::protocol::messages::{
    BatchDecision, BatchOffer, ChunkHeader, Hello, PairChallenge, PairResponse, PairReveal,
    TextDecision, TransferDecision, TransferOffer, TransferResult,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((Hello::Batch(batch), _)) = decode_frame::<Hello>(data) {
        let _ = batch.validate();
    }
    if let Ok((offer, used)) = decode_frame::<TransferOffer>(data) {
        assert!(used <= data.len());
        if offer.validate().is_ok() {
            let _ = offer.display_name();
            let _ = offer.chunk_len(offer.total_chunks().saturating_sub(1));
        }
    }
    if let Ok((batch, _)) = decode_frame::<BatchOffer>(data) {
        let _ = batch.validate();
    }
    let _ = decode_frame::<TransferDecision>(data);
    let _ = decode_frame::<ChunkHeader>(data);
    let _ = decode_frame::<TransferResult>(data);
    let _ = decode_frame::<BatchDecision>(data);
    let _ = decode_frame::<TextDecision>(data);
    let _ = decode_frame::<PairChallenge>(data);
    let _ = decode_frame::<PairReveal>(data);
    let _ = decode_frame::<PairResponse>(data);
});
