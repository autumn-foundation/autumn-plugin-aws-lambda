//! Pure time rules. Verus twins: `verification/core.rs`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Returns the milliseconds left for the upstream call.
///
/// The result is `deadline - now - margin`, or `0` when that is not positive.
#[must_use]
pub const fn invoke_budget_ms(deadline_ms: u64, now_ms: u64, margin_ms: u64) -> u64 {
    deadline_ms.saturating_sub(now_ms).saturating_sub(margin_ms)
}

/// Returns the next readiness delay: double `current`, capped at `max`.
///
/// If `current > max`, the result is `current`.
#[must_use]
pub const fn next_delay_ms(current: u64, max: u64) -> u64 {
    if current > max {
        current
    } else if current > max / 2 {
        max
    } else {
        current * 2
    }
}

/// Converts a duration to whole milliseconds. Saturates at `u64::MAX`.
pub fn as_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Returns the time left before `deadline_ms` (Unix ms), minus `margin`.
pub fn budget_until(deadline_ms: u64, margin: Duration) -> Duration {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, as_ms);
    Duration::from_millis(invoke_budget_ms(deadline_ms, now_ms, as_ms(margin)))
}
