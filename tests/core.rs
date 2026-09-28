//! Tests for the pure core: activation, timing, headers, upstream, failures.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use autumn_plugin_aws_lambda::{
    Activation, PluginError, ProxyFailure, REQUEST_ID_HEADER, Upstream, invoke_budget_ms,
    next_delay_ms, prepare_request_headers, strip_hop_by_hop,
};
use autumn_web::config::ServerConfig;
use http::{HeaderMap, HeaderValue, StatusCode};

// ---- Activation ----------------------------------------------------------

#[test]
fn auto_starts_only_on_lambda() {
    assert!(Activation::Auto.is_active(true));
    assert!(!Activation::Auto.is_active(false));
}

#[test]
fn always_and_never_ignore_the_environment() {
    for present in [true, false] {
        assert!(Activation::Always.is_active(present));
        assert!(!Activation::Never.is_active(present));
    }
}

#[test]
fn default_activation_is_auto() {
    assert_eq!(Activation::default(), Activation::Auto);
}

// ---- Timing --------------------------------------------------------------

#[test]
fn budget_is_deadline_minus_now_minus_margin() {
    assert_eq!(invoke_budget_ms(10_000, 4_000, 100), 5_900);
}

#[test]
fn budget_is_zero_at_or_after_deadline() {
    assert_eq!(invoke_budget_ms(10_000, 10_000, 0), 0);
    assert_eq!(invoke_budget_ms(10_000, 12_000, 0), 0);
}

#[test]
fn budget_is_zero_when_margin_uses_all_time() {
    assert_eq!(invoke_budget_ms(10_000, 9_950, 50), 0);
    assert_eq!(invoke_budget_ms(10_000, 9_950, 100), 0);
}

#[test]
fn budget_does_not_overflow_at_extremes() {
    assert_eq!(invoke_budget_ms(u64::MAX, 0, 0), u64::MAX);
    assert_eq!(invoke_budget_ms(0, u64::MAX, u64::MAX), 0);
}

#[test]
fn delay_doubles_then_caps() {
    assert_eq!(next_delay_ms(10, 1_000), 20);
    assert_eq!(next_delay_ms(600, 1_000), 1_000);
    assert_eq!(next_delay_ms(1_000, 1_000), 1_000);
}

#[test]
fn delay_above_max_does_not_change() {
    assert_eq!(next_delay_ms(2_000, 1_000), 2_000);
}

#[test]
fn delay_does_not_overflow() {
    assert_eq!(next_delay_ms(u64::MAX / 2 + 1, u64::MAX), u64::MAX);
}

// ---- Failures ------------------------------------------------------------

#[test]
fn failures_map_to_gateway_statuses() {
    assert_eq!(ProxyFailure::Connect.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(ProxyFailure::Body.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(ProxyFailure::Timeout.status(), StatusCode::GATEWAY_TIMEOUT);
}

// ---- Headers -------------------------------------------------------------

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(*name, HeaderValue::from_static(value));
    }
    map
}

#[test]
fn strip_removes_fixed_hop_by_hop_list() {
    let mut map = headers(&[
        ("connection", "keep-alive"),
        ("keep-alive", "timeout=5"),
        ("proxy-connection", "keep-alive"),
        ("transfer-encoding", "chunked"),
        ("te", "trailers"),
        ("trailer", "x-checksum"),
        ("upgrade", "websocket"),
        ("proxy-authenticate", "Basic"),
        ("proxy-authorization", "Basic abc"),
        ("content-type", "text/plain"),
    ]);
    strip_hop_by_hop(&mut map);
    assert_eq!(map.len(), 1);
    assert_eq!(map["content-type"], "text/plain");
}

#[test]
fn strip_removes_names_listed_in_connection() {
    let mut map = headers(&[
        ("connection", "close, X-Secret-Hop"),
        ("connection", "x-other"),
        ("x-secret-hop", "1"),
        ("x-other", "2"),
        ("x-keep", "3"),
    ]);
    strip_hop_by_hop(&mut map);
    assert!(map.get("x-secret-hop").is_none());
    assert!(map.get("x-other").is_none());
    assert_eq!(map["x-keep"], "3");
}

#[test]
fn strip_keeps_multi_value_end_to_end_headers() {
    let mut map = headers(&[("set-cookie", "a=1"), ("set-cookie", "b=2")]);
    strip_hop_by_hop(&mut map);
    let cookies: Vec<_> = map.get_all("set-cookie").iter().collect();
    assert_eq!(cookies, ["a=1", "b=2"]);
}

#[test]
fn strip_ignores_invalid_connection_tokens() {
    let mut map = headers(&[("connection", "bad token, ,"), ("x-keep", "1")]);
    strip_hop_by_hop(&mut map);
    assert_eq!(map.len(), 1);
}

#[test]
fn prepare_removes_content_length_and_sets_request_id() {
    let mut map = headers(&[
        ("content-length", "99"),
        ("host", "example.com"),
        ("connection", "close"),
    ]);
    prepare_request_headers(&mut map, Some("req-1"));
    assert!(map.get("content-length").is_none());
    assert!(map.get("connection").is_none());
    assert_eq!(map["host"], "example.com");
    assert_eq!(map[REQUEST_ID_HEADER], "req-1");
}

#[test]
fn prepare_keeps_existing_request_id() {
    let mut map = headers(&[("x-request-id", "client-id")]);
    prepare_request_headers(&mut map, Some("req-1"));
    assert_eq!(map[REQUEST_ID_HEADER], "client-id");
}

#[test]
fn prepare_skips_invalid_request_id() {
    let mut map = HeaderMap::new();
    prepare_request_headers(&mut map, Some("bad\nid"));
    assert!(map.get(REQUEST_ID_HEADER).is_none());
    prepare_request_headers(&mut map, None);
    assert!(map.get(REQUEST_ID_HEADER).is_none());
}

// ---- Upstream ------------------------------------------------------------

fn server(host: &str, port: u16) -> ServerConfig {
    ServerConfig {
        host: host.to_owned(),
        port,
        ..ServerConfig::default()
    }
}

#[test]
fn unspecified_ipv4_changes_to_loopback() {
    let up = Upstream::new(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 3000)));
    assert_eq!(up.addr(), SocketAddr::from((Ipv4Addr::LOCALHOST, 3000)));
}

#[test]
fn unspecified_ipv6_changes_to_loopback() {
    let up = Upstream::new(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 3000)));
    assert_eq!(up.addr(), SocketAddr::from((Ipv6Addr::LOCALHOST, 3000)));
}

#[test]
fn specific_ip_does_not_change() {
    let addr: SocketAddr = "10.0.0.5:8080".parse().expect("valid address");
    assert_eq!(Upstream::new(addr).addr(), addr);
}

#[test]
fn server_config_host_and_port_make_the_upstream() {
    let up = Upstream::from_server_config(&server("0.0.0.0", 8080)).expect("valid config");
    assert_eq!(up.addr(), SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)));
    assert_eq!(up.to_string(), "http://127.0.0.1:8080");
}

#[test]
fn server_config_accepts_localhost_and_ipv6() {
    let up = Upstream::from_server_config(&server("localhost", 3000)).expect("localhost");
    assert_eq!(up.addr(), SocketAddr::from((Ipv4Addr::LOCALHOST, 3000)));
    let up = Upstream::from_server_config(&server("::", 3000)).expect("ipv6");
    assert_eq!(up.addr(), SocketAddr::from((Ipv6Addr::LOCALHOST, 3000)));
    let up = Upstream::from_server_config(&server("[::1]", 3000)).expect("bracketed ipv6");
    assert_eq!(up.addr(), SocketAddr::from((Ipv6Addr::LOCALHOST, 3000)));
}

#[test]
fn server_config_default_is_loopback_3000() {
    let up = Upstream::from_server_config(&ServerConfig::default()).expect("default");
    assert_eq!(up.addr(), SocketAddr::from((Ipv4Addr::LOCALHOST, 3000)));
}

#[test]
fn server_config_rejects_hostname() {
    let err = Upstream::from_server_config(&server("app.internal", 3000)).expect_err("hostname");
    assert_eq!(
        err,
        PluginError::InvalidHost {
            host: "app.internal".to_owned()
        }
    );
}

#[test]
fn server_config_rejects_unix_socket() {
    let mut cfg = server("127.0.0.1", 3000);
    cfg.unix_socket = Some("/tmp/app.sock".to_owned());
    assert_eq!(
        Upstream::from_server_config(&cfg),
        Err(PluginError::UnixSocketUnsupported)
    );
}

#[test]
fn server_config_rejects_tls() {
    let mut cfg = server("127.0.0.1", 3000);
    cfg.tls = Some(
        serde_json::from_value(serde_json::json!({
            "cert_path": "/tmp/cert.pem",
            "key_path": "/tmp/key.pem"
        }))
        .expect("tls config"),
    );
    assert_eq!(
        Upstream::from_server_config(&cfg),
        Err(PluginError::TlsUnsupported)
    );
}

#[test]
fn server_config_rejects_zero_port() {
    assert_eq!(
        Upstream::from_server_config(&server("127.0.0.1", 0)),
        Err(PluginError::ZeroPort)
    );
}

// ---- Lambda environment ----------------------------------------------------

#[test]
fn missing_env_lists_required_lambda_variables() {
    let none = |_: &str| None;
    assert_eq!(
        autumn_plugin_aws_lambda::missing_lambda_env(none),
        [
            "AWS_LAMBDA_RUNTIME_API",
            "AWS_LAMBDA_FUNCTION_NAME",
            "AWS_LAMBDA_FUNCTION_MEMORY_SIZE",
            "AWS_LAMBDA_FUNCTION_VERSION",
        ]
    );
}

#[test]
fn missing_env_is_empty_on_lambda() {
    let all = |_: &str| Some("128".to_owned());
    assert!(autumn_plugin_aws_lambda::missing_lambda_env(all).is_empty());
}

#[test]
fn missing_env_rejects_bad_memory_size() {
    let env = |name: &str| {
        Some(if name == "AWS_LAMBDA_FUNCTION_MEMORY_SIZE" {
            "lots".to_owned()
        } else {
            "x".to_owned()
        })
    };
    assert_eq!(
        autumn_plugin_aws_lambda::missing_lambda_env(env),
        ["AWS_LAMBDA_FUNCTION_MEMORY_SIZE"]
    );
}

#[test]
fn missing_env_error_names_the_variables() {
    let err = PluginError::MissingEnv {
        names: vec!["AWS_LAMBDA_RUNTIME_API", "AWS_LAMBDA_FUNCTION_NAME"],
    };
    assert_eq!(
        err.to_string(),
        "Lambda environment is not complete; missing or invalid: AWS_LAMBDA_RUNTIME_API, AWS_LAMBDA_FUNCTION_NAME"
    );
}
