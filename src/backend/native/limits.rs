//! The server's size limits, which tests may shrink: pushing tens of MiB
//! through a PTY takes minutes on a loaded machine, while what the limits
//! do is the same at a few MiB.

use std::sync::OnceLock;

/// Divides every limit here, for tests; 1 (none) otherwise. Not passed
/// on to the shells.
pub const DIVISOR_VAR: &str = "PADDOCK_LIMITS_DIVISOR";

/// `bytes` divided by [`DIVISOR_VAR`]'s value, read once.
fn scaled(bytes: usize) -> usize {
    static DIVISOR: OnceLock<usize> = OnceLock::new();
    let divisor = *DIVISOR.get_or_init(|| {
        std::env::var(DIVISOR_VAR)
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&d| d >= 1)
            .unwrap_or(1)
    });
    bytes / divisor
}

/// The most input that may wait for a program: an input that would take
/// what waits past it is refused whole.
pub fn input_max() -> usize {
    scaled(super::terminal::INPUT_MAX)
}

/// The largest frame the server reads from a client.
pub fn frame_max() -> usize {
    scaled(super::protocol::FROM_CLIENT)
}

/// Bytes queued for an attached client past which it is behind.
pub fn behind() -> usize {
    scaled(super::state::BEHIND)
}

/// Bytes queued past which a client is let go at once.
pub fn max_queued() -> usize {
    scaled(super::state::MAX_QUEUED)
}
