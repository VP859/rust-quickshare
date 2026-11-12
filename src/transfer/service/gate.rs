//! Limits on the questions a user is asked: how many may wait at once, how
//! soon a declined device may ask to pair again, and which accepted files and
//! batches a broken connection may resume without asking again.

use crate::protocol::messages::TransferOffer;
use iroh::EndpointId;
use rust_i18n::t;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Upper bound for a handler to answer a question (a received text or a batch
/// of files); no answer counts as a rejection.
pub(crate) const QUESTION_TIMEOUT: Duration = Duration::from_secs(150);

/// Questions one device may have waiting for the user at the same time.
const MAX_OPEN_QUESTIONS_PER_PEER: usize = 2;

/// Questions all devices together may have waiting for the user. Node IDs are
/// free to generate, so this is the bound that keeps a flood of requests from
/// burying the user in dialogs.
const MAX_OPEN_QUESTIONS: usize = 8;

/// Pause after a declined or broken pairing request before the same device
/// may ask again.
const PAIR_RETRY_DELAY: Duration = Duration::from_secs(30);

/// How long an accepted file or batch stays accepted after its connection
/// broke, so the sender can resume without the user being asked again.
const RESUME_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Upper bound of remembered accepted files and batches.
const MAX_RESUMABLE: usize = 256;

/// Files of an accepted batch that may still arrive on a connection without
/// asking again. The sender cannot exceed what the user agreed to: neither
/// more files nor more bytes than announced.
#[derive(Debug)]
pub(super) struct BatchApproval {
    /// Files that may still be accepted automatically.
    remaining_files: u32,
    /// Bytes that may still be accepted automatically.
    remaining_bytes: u64,
    /// Folder chosen when the batch was accepted.
    dest_dir: PathBuf,
}

impl BatchApproval {
    /// Approval of `files` files with `bytes` bytes in total, saved to `dest_dir`.
    pub(super) fn new(files: u32, bytes: u64, dest_dir: PathBuf) -> Self {
        Self {
            remaining_files: files,
            remaining_bytes: bytes,
            dest_dir,
        }
    }

    /// Uses the approval for `offer`; returns the folder to save it to, or
    /// `None` when the offer exceeds what is left of the batch.
    pub(super) fn take(&mut self, offer: &TransferOffer) -> Option<PathBuf> {
        if self.remaining_files == 0 || offer.file_size > self.remaining_bytes {
            return None;
        }
        self.remaining_files -= 1;
        self.remaining_bytes -= offer.file_size;
        Some(self.dest_dir.clone())
    }
}

/// Rate limits for questions that need the user's attention (file offers,
/// batches, texts, pairing requests), shared by all connections of [`serve`](super::serve).
///
/// Requests over the limit are declined without asking, so another device
/// cannot flood the screen with dialogs or queue unlimited work. The gate also
/// remembers answers the user already gave for files and batches whose
/// connection broke, for [`RESUME_WINDOW`], so a resumed transfer does not ask
/// again.
#[derive(Debug, Default)]
pub(super) struct QuestionGate {
    /// Mutable state; held only for short, non-async sections.
    state: Mutex<GateState>,
}

/// State of [`QuestionGate`].
#[derive(Debug, Default)]
struct GateState {
    /// Open questions per device.
    open: HashMap<EndpointId, usize>,
    /// Open questions of all devices.
    total: usize,
    /// Devices that may not send a pairing request before the given instant.
    pair_blocked_until: HashMap<EndpointId, Instant>,
    /// Accepted files whose transfer broke: device and file hash → folder.
    resumable_files: HashMap<(EndpointId, [u8; 32]), (PathBuf, Instant)>,
    /// What was left of an accepted batch when its connection ended.
    resumable_batches: HashMap<EndpointId, (BatchApproval, Instant)>,
}

impl QuestionGate {
    /// Locks the state; a poisoned lock only means another task panicked while
    /// updating counters, which stay usable.
    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reserves a slot for a question from `peer`; `None` when it already has
    /// too many open questions or all devices together do. The slot is released
    /// when the permit is dropped.
    pub(super) fn enter(self: &Arc<Self>, peer: EndpointId) -> Option<QuestionPermit> {
        let mut state = self.lock();
        let open = state.open.get(&peer).copied().unwrap_or(0);
        if open >= MAX_OPEN_QUESTIONS_PER_PEER || state.total >= MAX_OPEN_QUESTIONS {
            return None;
        }
        state.open.insert(peer, open + 1);
        state.total += 1;
        Some(QuestionPermit {
            gate: self.clone(),
            peer,
        })
    }

    /// Whether `peer` may send a pairing request now.
    pub(super) fn pairing_allowed(&self, peer: &EndpointId) -> bool {
        let mut state = self.lock();
        let now = Instant::now();
        state.pair_blocked_until.retain(|_, until| *until > now);
        !state.pair_blocked_until.contains_key(peer)
    }

    /// Blocks pairing requests from `peer` for [`PAIR_RETRY_DELAY`].
    pub(super) fn block_pairing(&self, peer: EndpointId) {
        self.lock()
            .pair_blocked_until
            .insert(peer, Instant::now() + PAIR_RETRY_DELAY);
    }

    /// Remembers that the user accepted the file `hash` from `peer` into
    /// `dest_dir`; its transfer broke and may be resumed.
    pub(super) fn keep_file(&self, peer: EndpointId, hash: [u8; 32], dest_dir: PathBuf) {
        let mut state = self.lock();
        state.forget_expired();
        if state.resumable_files.len() < MAX_RESUMABLE {
            let until = Instant::now() + RESUME_WINDOW;
            state
                .resumable_files
                .insert((peer, hash), (dest_dir, until));
        }
    }

    /// Folder of an accepted file being resumed, if the user accepted it.
    pub(super) fn take_file(&self, peer: EndpointId, hash: &[u8; 32]) -> Option<PathBuf> {
        let mut state = self.lock();
        state.forget_expired();
        state
            .resumable_files
            .remove(&(peer, *hash))
            .map(|(dir, _)| dir)
    }

    /// Keeps the rest of an accepted batch when its connection ends.
    pub(super) fn keep_batch(&self, peer: EndpointId, approval: BatchApproval) {
        let mut state = self.lock();
        state.forget_expired();
        if approval.remaining_files > 0 && state.resumable_batches.len() < MAX_RESUMABLE {
            let until = Instant::now() + RESUME_WINDOW;
            state.resumable_batches.insert(peer, (approval, until));
        }
    }

    /// The rest of a batch `peer` started on an earlier connection.
    pub(super) fn take_batch(&self, peer: &EndpointId) -> Option<BatchApproval> {
        let mut state = self.lock();
        state.forget_expired();
        state
            .resumable_batches
            .remove(peer)
            .map(|(approval, _)| approval)
    }
}

impl GateState {
    /// Drops remembered answers older than [`RESUME_WINDOW`].
    fn forget_expired(&mut self) {
        let now = Instant::now();
        self.resumable_files.retain(|_, (_, until)| *until > now);
        self.resumable_batches.retain(|_, (_, until)| *until > now);
    }
}

/// Slot of one open question in a [`QuestionGate`], released on drop.
#[derive(Debug)]
pub(super) struct QuestionPermit {
    /// Gate the slot belongs to.
    gate: Arc<QuestionGate>,
    /// Device that asked the question.
    peer: EndpointId,
}

impl Drop for QuestionPermit {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        state.total = state.total.saturating_sub(1);
        if let Some(open) = state.open.get_mut(&self.peer) {
            *open = open.saturating_sub(1);
            if *open == 0 {
                state.open.remove(&self.peer);
            }
        }
    }
}

/// Reason sent back when a request is declined by the [`QuestionGate`].
pub(super) fn busy_reason() -> String {
    t!("service.busy").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(size: u64) -> TransferOffer {
        TransferOffer {
            file_name: "a.jpg".into(),
            file_size: size,
            chunk_size: 1024,
            file_hash: [0; 32],
            folder: Vec::new(),
        }
    }

    #[test]
    fn question_gate_limits_open_questions() {
        let gate = Arc::new(QuestionGate::default());
        let a = iroh::SecretKey::generate().public();
        let first = gate.enter(a).unwrap();
        let _second = gate.enter(a).unwrap();
        assert!(gate.enter(a).is_none(), "per-device limit");
        drop(first);
        assert!(gate.enter(a).is_some(), "slot released on drop");

        let others: Vec<_> = (0..MAX_OPEN_QUESTIONS)
            .filter_map(|_| gate.enter(iroh::SecretKey::generate().public()))
            .collect();
        assert_eq!(others.len(), MAX_OPEN_QUESTIONS - 1, "global limit");
        drop(others);
        assert_eq!(gate.lock().total, 1);
    }

    #[test]
    fn question_gate_delays_repeated_pairing() {
        let gate = QuestionGate::default();
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        assert!(gate.pairing_allowed(&a));
        gate.block_pairing(a);
        assert!(!gate.pairing_allowed(&a));
        assert!(gate.pairing_allowed(&b));
    }

    proptest::proptest! {
        /// Whatever a sender offers after an accepted batch, no more files and
        /// bytes are accepted than announced.
        #[test]
        fn batch_approval_holds_for_any_offers(
            files in 0u32..20,
            bytes in 0u64..10_000,
            sizes in proptest::collection::vec(0u64..4_000, 0..40),
        ) {
            let mut approval = BatchApproval {
                remaining_files: files,
                remaining_bytes: bytes,
                dest_dir: PathBuf::from("/tmp"),
            };
            let accepted: Vec<u64> = sizes.into_iter().filter(|s| approval.take(&offer(*s)).is_some()).collect();
            proptest::prop_assert!(accepted.len() as u64 <= u64::from(files));
            proptest::prop_assert!(accepted.iter().sum::<u64>() <= bytes);
        }
    }

    #[test]
    fn batch_approval_never_exceeds_the_announcement() {
        let mut approval = BatchApproval {
            remaining_files: 2,
            remaining_bytes: 100,
            dest_dir: PathBuf::from("/tmp"),
        };
        assert!(
            approval.take(&offer(101)).is_none(),
            "more bytes than announced"
        );
        assert!(approval.take(&offer(60)).is_some());
        assert!(
            approval.take(&offer(41)).is_none(),
            "over the remaining bytes"
        );
        assert!(approval.take(&offer(40)).is_some());
        assert!(
            approval.take(&offer(0)).is_none(),
            "more files than announced"
        );
    }
}
