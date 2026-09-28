//! Verus model of the pure core in `src/`.
//!
//! Each `exec` function here has the same logic as its runtime twin.
//! Tests in `tests/spec_conformance.rs` check that the twins agree.
//!
//! Run: `verification/verify.sh`.

use vstd::prelude::*;

verus! {

// ---------------------------------------------------------------------------
// Activation (twin: `src/activation.rs`)
// ---------------------------------------------------------------------------

/// Model of `Activation`.
pub enum Activation {
    Auto,
    Always,
    Never,
}

/// The loop starts only when this is true.
pub open spec fn spec_is_active(mode: Activation, runtime_api_present: bool) -> bool {
    match mode {
        Activation::Auto => runtime_api_present,
        Activation::Always => true,
        Activation::Never => false,
    }
}

/// Twin of `Activation::is_active`.
pub fn is_active(mode: Activation, runtime_api_present: bool) -> (r: bool)
    ensures
        r == spec_is_active(mode, runtime_api_present),
        // `Never` can never start the loop.
        mode is Never ==> !r,
        // `Auto` never starts the loop outside Lambda.
        mode is Auto && !runtime_api_present ==> !r,
{
    match mode {
        Activation::Auto => runtime_api_present,
        Activation::Always => true,
        Activation::Never => false,
    }
}

// ---------------------------------------------------------------------------
// Invocation budget (twin: `src/timing.rs::invoke_budget_ms`)
// ---------------------------------------------------------------------------

/// Time left for the upstream call, in milliseconds.
pub open spec fn spec_budget(deadline: u64, now: u64, margin: u64) -> int {
    if now as int + margin as int >= deadline as int {
        0
    } else {
        deadline as int - now as int - margin as int
    }
}

/// Twin of `invoke_budget_ms`.
pub fn invoke_budget_ms(deadline_ms: u64, now_ms: u64, margin_ms: u64) -> (r: u64)
    ensures
        r as int == spec_budget(deadline_ms, now_ms, margin_ms),
        // The call never ends after the deadline minus the margin.
        r > 0 ==> now_ms as int + r as int + margin_ms as int == deadline_ms as int,
        // No budget after the deadline.
        now_ms >= deadline_ms ==> r == 0,
        r <= deadline_ms,
{
    if now_ms >= deadline_ms {
        0
    } else {
        let left = deadline_ms - now_ms;
        if margin_ms >= left {
            0
        } else {
            left - margin_ms
        }
    }
}

// ---------------------------------------------------------------------------
// Readiness backoff (twin: `src/timing.rs::next_delay_ms`)
// ---------------------------------------------------------------------------

/// Twin of `next_delay_ms`. Doubles the delay, capped at `max`.
pub fn next_delay_ms(current: u64, max: u64) -> (r: u64)
    ensures
        r <= max || r == current,
        current <= max ==> r <= max,
        current <= max ==> r >= current,
        current > 0 ==> r > 0,
        current <= max / 2 ==> r == 2 * current,
{
    if current > max {
        current
    } else if current > max / 2 {
        max
    } else {
        current * 2
    }
}

/// Model of the readiness wait loop in `src/runtime.rs::wait_until`.
/// It returns the number of probes.
/// Proof: the loop ends, and the probe count has a fixed bound.
pub fn readiness_probes(timeout: u64, initial: u64, max: u64) -> (probes: u64)
    requires
        0 < initial <= max,
        timeout <= u64::MAX - max,
    ensures
        probes >= 1,
        probes as int <= timeout as int / initial as int + 2,
{
    let mut slept: u64 = 0;
    let mut delay: u64 = initial;
    let mut probes: u64 = 1;
    while slept < timeout
        invariant
            0 < initial <= delay <= max,
            timeout <= u64::MAX - max,
            slept <= timeout + max,
            probes >= 1,
            (probes as int - 1) * initial as int <= slept as int,
            probes as int <= timeout as int / initial as int + 2,
        decreases timeout + max - slept,
    {
        let ghost old_slept = slept as int;
        let ghost old_probes = probes as int;
        let ghost d = delay as int;
        let ghost t = timeout as int;
        let ghost init = initial as int;
        proof {
            // Before this probe, (old_probes - 1) probes each slept at least `initial`.
            assert((old_probes - 1) * init < t);
            assert(old_probes - 1 <= t / init) by (nonlinear_arith)
                requires
                    (old_probes - 1) * init < t,
                    init > 0,
                    old_probes >= 1,
            ;
        }
        slept = slept + delay;
        delay = next_delay_ms(delay, max);
        probes = probes + 1;
        proof {
            assert((probes as int - 1) * init <= slept as int) by (nonlinear_arith)
                requires
                    (old_probes - 1) * init <= old_slept,
                    d >= init,
                    probes as int == old_probes + 1,
                    slept as int == old_slept + d,
            ;
        }
    }
    probes
}

// ---------------------------------------------------------------------------
// Hop-by-hop filter (twin: `src/headers.rs::strip_hop_by_hop`)
//
// Header names are modeled as ids. `drop` is the fixed hop-by-hop list
// plus the names in the `Connection` header.
// ---------------------------------------------------------------------------

/// Twin of the membership check on the drop list.
pub fn member(x: u32, v: &Vec<u32>) -> (b: bool)
    ensures
        b == v@.contains(x),
{
    let mut i = 0;
    while i < v.len()
        invariant
            0 <= i <= v.len(),
            forall|k: int| 0 <= k < i ==> v@[k] != x,
        decreases v.len() - i,
    {
        if v[i] == x {
            assert(v@[i as int] == x);
            return true;
        }
        i += 1;
    }
    false
}

/// Twin of `strip_hop_by_hop`.
pub fn filter_headers(names: &Vec<u32>, drop: &Vec<u32>) -> (out: Vec<u32>)
    ensures
        // Safety: no hop-by-hop header goes out.
        forall|k: int| 0 <= k < out.len() ==> !drop@.contains(#[trigger] out@[k]),
        // No new headers appear.
        forall|k: int| 0 <= k < out.len() ==> names@.contains(#[trigger] out@[k]),
        // Liveness: every end-to-end header goes out.
        forall|k: int|
            0 <= k < names.len() && !drop@.contains(#[trigger] names@[k]) ==> out@.contains(
                names@[k],
            ),
{
    let mut out: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < names.len()
        invariant
            0 <= i <= names.len(),
            forall|k: int| 0 <= k < out.len() ==> !drop@.contains(#[trigger] out@[k]),
            forall|k: int| 0 <= k < out.len() ==> names@.contains(#[trigger] out@[k]),
            forall|k: int|
                0 <= k < i && !drop@.contains(#[trigger] names@[k]) ==> out@.contains(names@[k]),
        decreases names.len() - i,
    {
        let n = names[i];
        let ghost old_out = out@;
        if !member(n, drop) {
            out.push(n);
            proof {
                assert(out@[out@.len() - 1] == n);
                assert(names@[i as int] == n);
                assert forall|k: int| 0 <= k < out.len() implies names@.contains(
                    #[trigger] out@[k],
                ) by {
                    if k < old_out.len() {
                        assert(old_out[k] == out@[k]);
                        assert(names@.contains(old_out[k]));
                    } else {
                        assert(out@[k] == names@[i as int]);
                    }
                }
                assert forall|k: int|
                    0 <= k < i + 1 && !drop@.contains(#[trigger] names@[k]) implies out@.contains(
                    names@[k],
                ) by {
                    if k < i {
                        assert(old_out.contains(names@[k]));
                        let j = choose|j: int| 0 <= j < old_out.len() && old_out[j] == names@[k];
                        assert(out@[j] == names@[k]);
                    } else {
                        assert(out@[out@.len() - 1] == names@[k]);
                    }
                }
            }
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Upstream error status (twin: `src/proxy.rs::ProxyFailure::status`)
// ---------------------------------------------------------------------------

/// Model of `ProxyFailure`.
pub enum ProxyFailure {
    Connect,
    Timeout,
    Body,
}

/// Twin of `ProxyFailure::status`.
pub fn failure_status(f: ProxyFailure) -> (s: u16)
    ensures
        // Always a gateway error. Never a Lambda invocation error.
        s == 502 || s == 504,
        f is Timeout <==> s == 504,
{
    match f {
        ProxyFailure::Connect => 502,
        ProxyFailure::Timeout => 504,
        ProxyFailure::Body => 502,
    }
}

fn main() {
}

} // verus!
