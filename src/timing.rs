//! Pure time rules. Verus twins: `verification/core.rs`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Returns the milliseconds left for the upstream call.
///
/// The result is `deadline - now - margin`. If that is not positive, the
/// result is `0`.
#[must_use]
pub const fn invoke_budget_ms(deadline_ms: u64, now_ms: u64, margin_ms: u64) -> u64 {
    deadline_ms.saturating_sub(now_ms).saturating_sub(margin_ms)
}

/// Returns the next readiness delay. It is two times `current`, but not
/// more than `max`. If `current > max`, the result is `current`.
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

/// Returns `d` in whole milliseconds. If the value is too large, the
/// result is `u64::MAX`.
pub(crate) fn as_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Returns the time left before `deadline_ms` (Unix milliseconds), minus `margin`.
pub(crate) fn budget_until(deadline_ms: u64, margin: Duration) -> Duration {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, as_ms);
    Duration::from_millis(invoke_budget_ms(deadline_ms, now_ms, as_ms(margin)))
}
