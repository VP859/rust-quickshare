//! Node service shared by every user interface (daemon, TUI, GUI).
//!
//! [`serve`] accepts connections and dispatches incoming control streams:
//! file offers (one by one or announced together as a batch), downloads of
//! files shared by TICKET, pairing requests and texts for the clipboard.
//! [`send_files`], [`pull_shared`] and [`send_text`] are the outgoing
//! counterparts.
//!
//! An interface only implements [`TransferHandler`] (decisions about files and
//! batches, shared files, pairing confirmation, received texts) and renders the [`TransferEvent`]s it
//! receives.
//!
//! The parts, each in its own file: the events interfaces see
//! ([`TransferEvent`], [`TransferStatus`]), what they plug in
//! ([`TransferHandler`]), the limits on questions to the user, [`Tracker`]
//! (progress, pausing, cancelling), [`serve`] for incoming and [`send_files`]
//! and its siblings for outgoing transfers.

mod events;
mod gate;
mod handler;
mod incoming;
mod outgoing;
mod tracker;

pub use events::*;
pub(crate) use gate::QUESTION_TIMEOUT;
pub use handler::*;
pub use incoming::*;
pub use outgoing::*;
pub use tracker::*;

use crate::protocol::messages::ShareToken;

/// One-line preview of a text: line breaks and control characters become
/// spaces, repeated whitespace is collapsed and the result is cut to
/// `max_chars` characters with an ellipsis.
pub fn text_preview(text: &str, max_chars: usize) -> String {
    let flat = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let mut cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Generates a random share token.
pub fn new_share_token() -> ShareToken {
    *uuid::Uuid::new_v4().as_bytes()
}
