//! Pausing transfers and the speed limit.
//!
//! Every transfer runs inside [`Tracker::cancellable`](super::service::Tracker::cancellable),
//! which makes its pause switch available to the code that moves the data.
//! That code calls `pace` before each piece (a chunk, a body frame): it waits
//! while the transfer is paused, then while the speed limit of its direction
//! is used up. A paused receiver stops reading, so QUIC's (or TCP's) flow
//! control stops the sender as well; iroh's keep-alive keeps the connection
//! open meanwhile.
//!
//! The limit ([`set_speed_limit`]) applies to all transfers of the process
//! together, separately for sending and for receiving.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::watch;

use super::service::Direction;

tokio::task_local! {
    /// Pause switch of the transfer the current task belongs to.
    static PAUSE: watch::Receiver<bool>;
}

/// Runs `transfer` with `pause` as its pause switch (see [`pace`]).
pub(crate) async fn with_pause<F: std::future::Future>(
    pause: watch::Receiver<bool>,
    transfer: F,
) -> F::Output {
    PAUSE.scope(pause, transfer).await
}

/// A token bucket: bytes may be used at `rate` per second, with a burst of
/// one second's worth; using more creates a debt that is waited off.
struct Bucket {
    /// Bytes per second; 0 is unlimited.
    rate: u64,
    /// Bytes that may be used now (negative: debt).
    tokens: f64,
    /// When `tokens` was last refilled.
    last: Instant,
}

/// Buckets of sending and receiving.
static BUCKETS: Mutex<Option<[Bucket; 2]>> = Mutex::new(None);

/// Index of the bucket of `direction`.
fn slot(direction: Direction) -> usize {
    match direction {
        Direction::Upload => 0,
        Direction::Download => 1,
    }
}

/// Sets the speed limit in bytes per second for sending and, separately, for
/// receiving; `None` or `Some(0)` removes it. Applies to transfers in progress.
pub fn set_speed_limit(bytes_per_second: Option<u64>) {
    let rate = bytes_per_second.unwrap_or(0);
    let mut buckets = BUCKETS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    let bucket = || Bucket {
        rate,
        tokens: rate as f64,
        last: now,
    };
    *buckets = (rate > 0).then(|| [bucket(), bucket()]);
}

/// The current speed limit in bytes per second, if any.
pub fn speed_limit() -> Option<u64> {
    BUCKETS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|b| b[0].rate)
}

/// Waits while the current transfer is paused, then takes `bytes` from the
/// speed limit of `direction`, waiting when it is used up.
pub(crate) async fn pace(direction: Direction, bytes: u64) {
    if let Ok(mut pause) = PAUSE.try_with(Clone::clone) {
        // `wait_for` returns at once when not paused, and with an error when
        // the transfer ended (its switch was dropped).
        let _ = pause.wait_for(|paused| !paused).await;
    }
    let wait = {
        let mut buckets = BUCKETS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(bucket) = buckets.as_mut().map(|b| &mut b[slot(direction)]) else {
            return;
        };
        let now = Instant::now();
        let rate = bucket.rate as f64;
        bucket.tokens =
            (bucket.tokens + now.duration_since(bucket.last).as_secs_f64() * rate).min(rate);
        bucket.last = now;
        bucket.tokens -= bytes as f64;
        (bucket.tokens < 0.0).then(|| Duration::from_secs_f64(-bucket.tokens / rate))
    };
    if let Some(wait) = wait {
        tokio::time::sleep(wait).await;
    }
}

/// Parses a speed like `500K`, `2M`, `1.5MB/s`, `100000` (bytes per second;
/// `K` and `M` are 1024-based) or `off`/`0` (no limit).
///
/// # Errors
///
/// Fails for anything else.
pub fn parse_speed(text: &str) -> anyhow::Result<Option<u64>> {
    let text = text.trim().to_ascii_lowercase();
    let text = text.trim_end_matches("/s").trim_end_matches('b').trim();
    if text.is_empty() || text == "off" || text == "0" || text == "none" {
        return Ok(None);
    }
    let (number, unit) = match text.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        Some((i, _)) => (&text[..i], &text[i..]),
        None => (text, ""),
    };
    let factor = match unit.trim() {
        "" => 1.0,
        "k" | "ki" => 1024.0,
        "m" | "mi" => 1024.0 * 1024.0,
        "g" | "gi" => 1024.0 * 1024.0 * 1024.0,
        _ => anyhow::bail!(rust_i18n::t!("control.bad_speed", value = text)),
    };
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!(rust_i18n::t!("control.bad_speed", value = text)))?;
    let bytes = (value * factor).round();
    if !(1.0..=1e12).contains(&bytes) {
        anyhow::bail!(rust_i18n::t!("control.bad_speed", value = text));
    }
    Ok(Some(bytes as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_are_parsed() {
        assert_eq!(parse_speed("off").unwrap(), None);
        assert_eq!(parse_speed("0").unwrap(), None);
        assert_eq!(parse_speed("500K").unwrap(), Some(500 * 1024));
        assert_eq!(parse_speed("2M").unwrap(), Some(2 * 1024 * 1024));
        assert_eq!(parse_speed("1.5 MB/s").unwrap(), Some(1_572_864));
        assert_eq!(parse_speed("100000").unwrap(), Some(100_000));
        for bad in ["fast", "-1M", "2X", "0.1"] {
            assert!(parse_speed(bad).is_err(), "{bad}");
        }
    }
}
