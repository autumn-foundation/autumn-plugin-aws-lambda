//! Property tests: the runtime code agrees with the Verus spec in
//! `verification/core.rs`. Proofs cover the model. These tests connect
//! the model to the real code.

use std::collections::HashSet;

use autumn_plugin_aws_lambda::{invoke_budget_ms, next_delay_ms, strip_hop_by_hop};
use http::{HeaderMap, HeaderName, HeaderValue};
use proptest::prelude::*;

/// `spec_budget` from the Verus model.
fn spec_budget(deadline: u64, now: u64, margin: u64) -> i128 {
    let (d, n, m) = (i128::from(deadline), i128::from(now), i128::from(margin));
    if n + m >= d { 0 } else { d - n - m }
}

const FIXED_HOP: [&str; 8] = [
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "proxy-authenticate",
    "proxy-authorization",
];

fn header_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("keep-alive".to_owned()),
        Just("transfer-encoding".to_owned()),
        Just("upgrade".to_owned()),
        Just("te".to_owned()),
        "x-[a-c]{1,2}",
        Just("content-type".to_owned()),
        Just("set-cookie".to_owned()),
    ]
}

proptest! {
    #[test]
    fn budget_matches_spec(deadline: u64, now: u64, margin: u64) {
        let r = invoke_budget_ms(deadline, now, margin);
        prop_assert_eq!(i128::from(r), spec_budget(deadline, now, margin));
        if r > 0 {
            prop_assert_eq!(i128::from(now) + i128::from(r) + i128::from(margin), i128::from(deadline));
        }
    }

    #[test]
    fn delay_matches_spec(current: u64, max: u64) {
        let r = next_delay_ms(current, max);
        prop_assert!(r <= max || r == current);
        if current <= max {
            prop_assert!(r <= max && r >= current);
        }
        if current > 0 {
            prop_assert!(r > 0);
        }
        if current <= max / 2 {
            prop_assert_eq!(r, 2 * current);
        }
    }

    #[test]
    fn filter_is_safe_and_live(
        names in proptest::collection::vec(header_name(), 0..12),
        listed in proptest::collection::vec("x-[a-c]{1,2}", 0..3),
    ) {
        let mut map = HeaderMap::new();
        for (i, name) in names.iter().enumerate() {
            let name = HeaderName::from_bytes(name.as_bytes()).expect("valid name");
            map.append(name, HeaderValue::from(i));
        }
        if !listed.is_empty() {
            map.append("connection", HeaderValue::from_str(&listed.join(", ")).expect("valid value"));
        }
        let mut drop: HashSet<String> = FIXED_HOP.iter().map(|s| (*s).to_owned()).collect();
        drop.insert("connection".to_owned());
        drop.extend(listed.iter().cloned());

        let before = map.clone();
        strip_hop_by_hop(&mut map);

        // Safety: no dropped name goes out.
        for name in map.keys() {
            prop_assert!(!drop.contains(name.as_str()), "leaked {name}");
        }
        // No new headers, and every end-to-end value stays, in order.
        for name in before.keys() {
            if drop.contains(name.as_str()) {
                continue;
            }
            let want: Vec<_> = before.get_all(name).iter().collect();
            let got: Vec<_> = map.get_all(name).iter().collect();
            prop_assert_eq!(want, got);
        }
        prop_assert!(map.keys().all(|k| before.contains_key(k)));
    }
}
