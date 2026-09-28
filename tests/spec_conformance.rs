//! Property tests: the runtime code agrees with the Verus spec in
//! `verification/core.rs`. Proofs cover the model. These tests connect
//! the model to the real code.

use std::collections::HashSet;

use autumn_plugin_aws_lambda::__private::{
    fold_repeated_headers, invoke_budget_ms, next_delay_ms, strip_hop_by_hop,
    upstream_path_and_query,
};
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

/// `is_path_byte` from the Verus model.
fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b)
}

/// `safe_path` from the Verus model.
fn safe_path(out: &[u8]) -> bool {
    out.iter().enumerate().all(|(k, &b)| {
        is_path_byte(b)
            || (b == b'%'
                && out.get(k + 1).is_some_and(u8::is_ascii_hexdigit)
                && out.get(k + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

/// A token for the `Connection` header: any generated name, in any case.
fn listed_token() -> impl Strategy<Value = String> {
    (header_name(), any::<bool>()).prop_map(|(name, upper)| {
        if upper {
            name.to_ascii_uppercase()
        } else {
            name
        }
    })
}

/// Copy of the `next_delay_ms` exec body in the Verus model.
const fn model_next_delay(current: u64, max: u64) -> u64 {
    if current > max {
        current
    } else if current > max / 2 {
        max
    } else {
        current * 2
    }
}

proptest! {
    #[test]
    fn delay_equals_the_model_for_small_values(current in 0u64..300, max in 0u64..300) {
        prop_assert_eq!(next_delay_ms(current, max), model_next_delay(current, max));
    }

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
    fn filter_is_safe_and_complete(
        names in proptest::collection::vec(header_name(), 0..12),
        listed in proptest::collection::vec(listed_token(), 0..3),
        lines in 1usize..3,
        sep in prop_oneof![Just(","), Just(", "), Just(" ,\t"), Just(",,")],
    ) {
        let mut map = HeaderMap::new();
        for (i, name) in names.iter().enumerate() {
            let name = HeaderName::from_bytes(name.as_bytes()).expect("valid name");
            map.append(name, HeaderValue::from(i));
        }
        // Spread the listed tokens over several `Connection` lines.
        for chunk in listed.chunks(listed.len().div_ceil(lines).max(1)) {
            let value = chunk.join(sep);
            map.append("connection", HeaderValue::from_str(&value).expect("valid value"));
        }
        // Oracle: RFC 9110 names are case-insensitive tokens.
        let mut drop: HashSet<String> = FIXED_HOP.iter().map(|s| (*s).to_owned()).collect();
        drop.insert("connection".to_owned());
        drop.extend(listed.iter().map(|t| t.to_ascii_lowercase()));

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

    #[test]
    fn fold_leaves_one_joined_value_per_name(
        names in proptest::collection::vec(header_name(), 0..12),
    ) {
        let mut map = HeaderMap::new();
        for (i, name) in names.iter().enumerate() {
            let name = HeaderName::from_bytes(name.as_bytes()).expect("valid name");
            map.append(name, HeaderValue::from(i));
        }
        let before = map.clone();
        fold_repeated_headers(&mut map);
        prop_assert_eq!(map.keys_len(), before.keys_len());
        for name in before.keys() {
            let values: Vec<&str> = before
                .get_all(name)
                .iter()
                .map(|v| v.to_str().expect("ascii"))
                .collect();
            let got: Vec<&str> = map.get_all(name).iter().map(|v| v.to_str().expect("ascii")).collect();
            if name == "set-cookie" {
                prop_assert_eq!(got, values);
            } else {
                let joined = values.join(", ");
                prop_assert_eq!(got, vec![joined.as_str()]);
            }
        }
    }

    #[test]
    fn encoded_path_is_safe_and_parses(raw in "[ -~é%?#]{0,40}|\\PC{0,20}") {
        let uri: http::Uri = "/".parse().expect("uri");
        let out = upstream_path_and_query(Some(&raw), &uri);
        prop_assert!(out.starts_with('/'));
        prop_assert!(safe_path(out.as_bytes()), "{out}");
        prop_assert!(out.parse::<http::uri::PathAndQuery>().is_ok(), "{out}");
    }
}
