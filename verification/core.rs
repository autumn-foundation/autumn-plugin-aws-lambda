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
        // `Never` does not start the loop.
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
        // The budget ends exactly at the deadline minus the margin.
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

/// Model of the readiness wait loop in `src/runtime.rs::wait_until`, for
/// the case where `ready` stays false. It returns the number of probes and
/// the total sleep time. The runtime normalizes `initial` and `max` so that
/// the precondition is true. The runtime measures wall-clock time, which
/// is not less than the sum of sleeps, so the runtime makes no more probes.
/// Proof: the loop ends, the last sleep stops at `timeout`, and the probe
/// count has a fixed bound.
pub fn readiness_probes(timeout: u64, initial: u64, max: u64) -> (res: (u64, u64))
    requires
        0 < initial <= max,
        // This precondition keeps the model counter in range.
        timeout < u64::MAX - 1,
    ensures
        res.0 >= 1,
        res.0 as int <= timeout as int / initial as int + 2,
        // The wait ends at the timeout, not later.
        res.1 == timeout,
{
    let mut slept: u64 = 0;
    let mut delay: u64 = initial;
    let mut probes: u64 = 1;
    while slept < timeout
        invariant
            0 < initial <= delay <= max,
            timeout < u64::MAX - 1,
            slept <= timeout,
            probes >= 1,
            slept < timeout ==> (probes as int - 1) * initial as int <= slept as int,
            probes as int <= timeout as int / initial as int + 2,
        decreases timeout - slept,
    {
        let ghost old_slept = slept as int;
        let ghost old_probes = probes as int;
        let ghost t = timeout as int;
        let ghost init = initial as int;
        proof {
            // Each earlier probe sleeps at least `initial`, and `slept < timeout`.
            assert(old_probes - 1 <= t / init) by (nonlinear_arith)
                requires
                    (old_probes - 1) * init <= old_slept,
                    old_slept < t,
                    init > 0,
                    old_probes >= 1,
            ;
            assert(t / init <= t) by (nonlinear_arith)
                requires
                    t >= 0,
                    init > 0,
            ;
            assert(old_probes < u64::MAX);
        }
        let step = if delay < timeout - slept { delay } else { timeout - slept };
        slept = slept + step;
        delay = next_delay_ms(delay, max);
        probes = probes + 1;
        proof {
            if slept < timeout {
                // This is not the last sleep. Thus the step is the full delay.
                assert((probes as int - 1) * init <= slept as int) by (nonlinear_arith)
                    requires
                        (old_probes - 1) * init <= old_slept,
                        step as int >= init,
                        probes as int == old_probes + 1,
                        slept as int == old_slept + step as int,
                ;
            }
        }
    }
    (probes, slept)
}

// ---------------------------------------------------------------------------
// Hop-by-hop filter (twin: `src/headers.rs::strip_hop_by_hop`)
//
// The model uses integer IDs for header names. For responses, `drop` is
// the fixed hop-by-hop list and the names in `Connection`. For requests,
// `drop` is the fixed list and the forwarding headers.
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
        // Completeness: every end-to-end header goes out.
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

// ---------------------------------------------------------------------------
// Zero budget (twin: `src/proxy.rs::LambdaProxy::forward`)
// ---------------------------------------------------------------------------

/// Twin of the rule in `forward`: with no budget, do not call the upstream.
pub fn calls_upstream(deadline_ms: u64, now_ms: u64, margin_ms: u64) -> (r: bool)
    ensures
        r <==> spec_budget(deadline_ms, now_ms, margin_ms) > 0,
{
    invoke_budget_ms(deadline_ms, now_ms, margin_ms) > 0
}

// ---------------------------------------------------------------------------
// Path encoding (twin: `src/headers.rs::encode_path`)
// ---------------------------------------------------------------------------

/// RFC 3986 `pchar` and `/`, but not `%`.
pub open spec fn is_path_byte(b: u8) -> bool {
    (0x30 <= b <= 0x39) || (0x41 <= b <= 0x5a) || (0x61 <= b <= 0x7a) || b == 0x2d || b == 0x2e
        || b == 0x5f || b == 0x7e || b == 0x21 || b == 0x24 || b == 0x26 || b == 0x27 || b
        == 0x28 || b == 0x29 || b == 0x2a || b == 0x2b || b == 0x2c || b == 0x3b || b == 0x3d
        || b == 0x3a || b == 0x40 || b == 0x2f
}

pub open spec fn is_hex(b: u8) -> bool {
    (0x30 <= b <= 0x39) || (0x41 <= b <= 0x46) || (0x61 <= b <= 0x66)
}

/// Safety: each byte is a path byte or `%`, and each `%` starts a
/// complete `%XX` escape. So `?`, `#`, space, CR, LF, and non-ASCII bytes
/// cannot reach the request line.
pub open spec fn safe_path(out: Seq<u8>) -> bool {
    &&& forall|k: int| 0 <= k < out.len() ==> is_path_byte(#[trigger] out[k]) || out[k] == 0x25
    &&& forall|k: int|
        0 <= k < out.len() && #[trigger] out[k] == 0x25 ==> k + 2 < out.len() && is_hex(out[k + 1])
            && is_hex(out[k + 2])
}

fn is_path_byte_exec(b: u8) -> (r: bool)
    ensures
        r == is_path_byte(b),
{
    (0x30 <= b && b <= 0x39) || (0x41 <= b && b <= 0x5a) || (0x61 <= b && b <= 0x7a) || b == 0x2d
        || b == 0x2e || b == 0x5f || b == 0x7e || b == 0x21 || b == 0x24 || b == 0x26 || b == 0x27
        || b == 0x28 || b == 0x29 || b == 0x2a || b == 0x2b || b == 0x2c || b == 0x3b || b == 0x3d
        || b == 0x3a || b == 0x40 || b == 0x2f
}

fn is_hex_exec(b: u8) -> (r: bool)
    ensures
        r == is_hex(b),
{
    (0x30 <= b && b <= 0x39) || (0x41 <= b && b <= 0x46) || (0x61 <= b && b <= 0x66)
}

/// Twin of the `HEX` table lookup.
fn hex_digit(n: u8) -> (r: u8)
    requires
        n < 16,
    ensures
        is_hex(r),
        is_path_byte(r),
{
    if n < 10 {
        0x30 + n
    } else {
        0x41 + (n - 10)
    }
}

/// Twin of `encode_path`.
pub fn encode_path(raw: &Vec<u8>) -> (out: Vec<u8>)
    ensures
        safe_path(out@),
{
    let mut out: Vec<u8> = Vec::new();
    let mut i: usize = 0;
    while i < raw.len()
        invariant
            0 <= i <= raw.len(),
            safe_path(out@),
        decreases raw.len() - i,
    {
        let b = raw[i];
        let ghost old = out@;
        if b == 0x25 && raw.len() - i > 2 && is_hex_exec(raw[i + 1]) && is_hex_exec(raw[i + 2]) {
            out.push(b);
            out.push(raw[i + 1]);
            out.push(raw[i + 2]);
            proof {
                assert(out@ =~= old.push(b).push(raw@[i + 1]).push(raw@[i + 2]));
                lemma_append_escape(old, b, raw@[i as int + 1], raw@[i as int + 2]);
            }
            i += 3;
        } else if is_path_byte_exec(b) {
            out.push(b);
            proof {
                assert(out@ =~= old.push(b));
                lemma_append_path_byte(old, b);
            }
            i += 1;
        } else {
            let hi = hex_digit(b / 16);
            let lo = hex_digit(b % 16);
            out.push(0x25);
            out.push(hi);
            out.push(lo);
            proof {
                assert(out@ =~= old.push(0x25).push(hi).push(lo));
                lemma_append_escape(old, 0x25, hi, lo);
            }
            i += 1;
        }
    }
    out
}

proof fn lemma_append_path_byte(s: Seq<u8>, b: u8)
    requires
        safe_path(s),
        is_path_byte(b),
    ensures
        safe_path(s.push(b)),
{
    let t = s.push(b);
    assert forall|k: int| 0 <= k < t.len() implies is_path_byte(#[trigger] t[k]) || t[k] == 0x25 by {
        if k < s.len() {
            assert(t[k] == s[k]);
        }
    }
    assert forall|k: int|
        0 <= k < t.len() && #[trigger] t[k] == 0x25 implies k + 2 < t.len() && is_hex(t[k + 1])
        && is_hex(t[k + 2]) by {
        if k < s.len() {
            assert(t[k] == s[k]);
            assert(s[k] == 0x25);
            assert(t[k + 1] == s[k + 1]);
            assert(t[k + 2] == s[k + 2]);
        } else {
            assert(t[k] == b);
            assert(!is_path_byte(0x25));
        }
    }
}

proof fn lemma_append_escape(s: Seq<u8>, p: u8, h1: u8, h2: u8)
    requires
        safe_path(s),
        p == 0x25,
        is_hex(h1),
        is_hex(h2),
    ensures
        safe_path(s.push(p).push(h1).push(h2)),
{
    let t = s.push(p).push(h1).push(h2);
    let n = s.len() as int;
    assert(t[n] == p && t[n + 1] == h1 && t[n + 2] == h2);
    assert forall|k: int| 0 <= k < t.len() implies is_path_byte(#[trigger] t[k]) || t[k] == 0x25 by {
        if k < n {
            assert(t[k] == s[k]);
        }
    }
    assert forall|k: int|
        0 <= k < t.len() && #[trigger] t[k] == 0x25 implies k + 2 < t.len() && is_hex(t[k + 1])
        && is_hex(t[k + 2]) by {
        if k < n {
            assert(t[k] == s[k]);
            assert(s[k] == 0x25);
            assert(t[k + 1] == s[k + 1]);
            assert(t[k + 2] == s[k + 2]);
        } else if k == n {
        } else {
            // Hex digits are not `%`.
            assert(k == n + 1 || k == n + 2);
        }
    }
}

fn main() {
}

} // verus!
