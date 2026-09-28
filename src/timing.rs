//! Pure time rules. Verus twins: `verification/core.rs`.

/// Returns the milliseconds left for the upstream call.
///
/// The result is `deadline - now - margin`, or `0` when that is not positive.
#[must_use]
pub const fn invoke_budget_ms(deadline_ms: u64, now_ms: u64, margin_ms: u64) -> u64 {
    let _ = (deadline_ms, now_ms, margin_ms);
    todo!()
}

/// Returns the next readiness delay: double `current`, capped at `max`.
///
/// If `current > max`, the result is `current`.
#[must_use]
pub const fn next_delay_ms(current: u64, max: u64) -> u64 {
    let _ = (current, max);
    todo!()
}
