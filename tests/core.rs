//! Tests for the pure core: activation, timing, headers, upstream, failures.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use autumn_plugin_aws_lambda::__private::{
    LAMBDA_REQUEST_ID_HEADER, ProxyFailure, Upstream, fold_repeated_headers, forwarded_client_ip,
    invoke_budget_ms, lambda_concurrency, missing_lambda_env, next_delay_ms,
    prepare_request_headers, slow_shutdown_settings, strip_hop_by_hop, text_body_is_lossless,
    upstream_path_and_query,
};
use autumn_plugin_aws_lambda::PluginError;
use autumn_web::config::ServerConfig;
use http::{HeaderMap, HeaderValue, StatusCode};

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
fn strip_reads_connection_values_with_non_ascii_bytes() {
    let mut map = HeaderMap::new();
    map.append(
        "connection",
        HeaderValue::from_bytes(b"x-secret, caf\xe9").expect("value"),
    );
    map.append("connection", HeaderValue::from_static("X-Upper,\tx-tab"));
    map.append("x-secret", HeaderValue::from_static("1"));
    map.append("x-upper", HeaderValue::from_static("2"));
    map.append("x-tab", HeaderValue::from_static("3"));
    strip_hop_by_hop(&mut map);
    assert!(map.is_empty(), "{map:?}");
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
    assert_eq!(map[LAMBDA_REQUEST_ID_HEADER], "req-1");
}

#[test]
fn prepare_replaces_a_client_request_id() {
    let mut map = headers(&[
        ("lambda-runtime-aws-request-id", "spoofed"),
        ("lambda-runtime-aws-request-id", "spoofed-2"),
    ]);
    prepare_request_headers(&mut map, Some("req-1"));
    let ids: Vec<_> = map.get_all(LAMBDA_REQUEST_ID_HEADER).iter().collect();
    assert_eq!(ids, ["req-1"]);
}

#[test]
fn prepare_removes_a_client_request_id_without_a_lambda_id() {
    let mut map = headers(&[("lambda-runtime-aws-request-id", "spoofed")]);
    prepare_request_headers(&mut map, Some("bad\nid"));
    assert!(map.get(LAMBDA_REQUEST_ID_HEADER).is_none());
    let mut map = headers(&[("lambda-runtime-aws-request-id", "spoofed")]);
    prepare_request_headers(&mut map, None);
    assert!(map.get(LAMBDA_REQUEST_ID_HEADER).is_none());
}

#[test]
fn fold_joins_repeated_headers_in_order() {
    let mut map = headers(&[
        ("vary", "origin"),
        ("vary", "accept-encoding"),
        ("x-one", "1"),
    ]);
    fold_repeated_headers(&mut map);
    let vary: Vec<_> = map.get_all("vary").iter().collect();
    assert_eq!(vary, ["origin, accept-encoding"]);
    assert_eq!(map["x-one"], "1");
}

#[test]
fn fold_keeps_each_set_cookie() {
    let mut map = headers(&[
        ("set-cookie", "a=1; Expires=Wed, 21 Oct 2026 07:28:00 GMT"),
        ("set-cookie", "b=2"),
    ]);
    fold_repeated_headers(&mut map);
    assert_eq!(map.get_all("set-cookie").iter().count(), 2);
}

// ---- Upstream path -------------------------------------------------------------

#[test]
fn raw_path_is_used_without_normalization() {
    let uri: http::Uri = "https://h/admin".parse().expect("uri");
    assert_eq!(
        upstream_path_and_query(Some("/public/%2e%2e/admin"), &uri),
        "/public/%2e%2e/admin"
    );
}

#[test]
fn raw_path_drops_the_stage_prefix_that_lambda_http_adds() {
    // REST API: lambda_http adds `/prod`; the raw path has no stage.
    let uri: http::Uri = "https://h/prod/hello?a=1&b=2".parse().expect("uri");
    assert_eq!(
        upstream_path_and_query(Some("/hello"), &uri),
        "/hello?a=1&b=2"
    );
}

#[test]
fn raw_path_is_percent_encoded_where_needed() {
    let uri: http::Uri = "https://h/".parse().expect("uri");
    assert_eq!(
        upstream_path_and_query(Some("/a b/é?#/100%/%41"), &uri),
        "/a%20b/%C3%A9%3F%23/100%25/%41"
    );
}

#[test]
fn missing_or_relative_raw_path_is_fixed() {
    let uri: http::Uri = "https://h/from-uri?q=1".parse().expect("uri");
    assert_eq!(upstream_path_and_query(None, &uri), "/from-uri?q=1");
    assert_eq!(upstream_path_and_query(Some(""), &uri), "/?q=1");
    assert_eq!(upstream_path_and_query(Some("x"), &uri), "/x?q=1");
}

// ---- Client IP -----------------------------------------------------------------

#[test]
fn source_ip_from_the_request_context_wins() {
    let ip = forwarded_client_ip(Some("203.0.113.9"), Some("1.1.1.1, 2.2.2.2"));
    assert_eq!(ip, Some("203.0.113.9".parse().expect("ip")));
}

#[test]
fn last_forwarded_for_entry_is_used_without_a_source_ip() {
    // ALB appends the real peer as the last entry.
    let ip = forwarded_client_ip(None, Some("1.1.1.1, 2001:db8::1"));
    assert_eq!(ip, Some("2001:db8::1".parse().expect("ip")));
}

#[test]
fn bad_client_ip_values_give_none() {
    assert_eq!(forwarded_client_ip(Some("not-an-ip"), None), None);
    assert_eq!(forwarded_client_ip(None, Some("1.1.1.1, junk")), None);
    assert_eq!(forwarded_client_ip(None, None), None);
}

#[test]
fn prepare_keeps_connection_listed_request_headers() {
    // The event is not a hop. A client must not delete headers with Connection.
    let mut map = headers(&[
        ("connection", "keep-alive, host, x-forwarded-for"),
        ("host", "example.com"),
        ("x-forwarded-for", "203.0.113.9"),
    ]);
    prepare_request_headers(&mut map, None);
    assert!(map.get("connection").is_none());
    assert_eq!(map["host"], "example.com");
}

#[test]
fn prepare_drops_client_forwarding_headers() {
    let mut map = headers(&[
        ("x-forwarded-host", "victim.example.com"),
        ("x-real-ip", "10.0.0.1"),
        ("forwarded", "for=10.0.0.1"),
        ("x-forwarded-proto", "https"),
    ]);
    prepare_request_headers(&mut map, None);
    assert!(map.get("x-forwarded-host").is_none());
    assert!(map.get("x-real-ip").is_none());
    assert!(map.get("forwarded").is_none());
    assert_eq!(map["x-forwarded-proto"], "https");
}

// ---- Text bodies -------------------------------------------------------------

#[test]
fn utf8_and_ascii_text_is_lossless() {
    assert!(text_body_is_lossless(
        Some("text/plain"),
        "héllo".as_bytes()
    ));
    assert!(text_body_is_lossless(
        Some("text/html; charset=utf-8"),
        b"<p>"
    ));
    assert!(text_body_is_lossless(None, b"plain"));
    assert!(text_body_is_lossless(
        Some("text/csv; charset=windows-1252"),
        b"only ascii"
    ));
}

#[test]
fn other_charsets_and_invalid_utf8_are_not_lossless() {
    assert!(!text_body_is_lossless(
        Some("text/csv; charset=windows-1252"),
        b"caf\xe9"
    ));
    assert!(!text_body_is_lossless(Some("text/plain"), b"\xff\xfe"));
    assert!(!text_body_is_lossless(
        Some("text/plain; charset=utf-16"),
        "hé".as_bytes()
    ));
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
    assert!(matches!(err, PluginError::InvalidHost { ref host, .. } if host == "app.internal"));
}

#[test]
fn server_config_rejects_unix_socket() {
    let mut cfg = server("127.0.0.1", 3000);
    cfg.unix_socket = Some("/tmp/app.sock".to_owned());
    assert!(matches!(
        Upstream::from_server_config(&cfg),
        Err(PluginError::UnixSocketUnsupported)
    ));
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
    assert!(matches!(
        Upstream::from_server_config(&cfg),
        Err(PluginError::TlsUnsupported)
    ));
}

#[test]
fn server_config_rejects_zero_port() {
    assert!(matches!(
        Upstream::from_server_config(&server("127.0.0.1", 0)),
        Err(PluginError::ZeroPort)
    ));
}

// ---- Lambda environment ----------------------------------------------------

#[test]
fn missing_env_lists_required_lambda_variables() {
    let none = |_: &str| None;
    assert_eq!(
        missing_lambda_env(none),
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
    assert!(missing_lambda_env(all).is_empty());
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
    assert_eq!(missing_lambda_env(env), ["AWS_LAMBDA_FUNCTION_MEMORY_SIZE"]);
}

// ---- Errors ------------------------------------------------------------------

#[test]
fn errors_have_clear_messages() {
    assert!(
        PluginError::ZeroPort
            .to_string()
            .contains("server.port is 0")
    );
    assert!(
        PluginError::TlsUnsupported
            .to_string()
            .contains("server.tls")
    );
    assert!(
        PluginError::UnixSocketUnsupported
            .to_string()
            .contains("server.unix_socket")
    );
    let io = std::io::Error::other("no threads");
    assert!(
        PluginError::Runtime(io)
            .to_string()
            .contains("cannot start the Lambda runtime thread: no threads")
    );
}

#[test]
fn errors_convert_to_autumn_errors() {
    let err: autumn_web::AutumnError = PluginError::ZeroPort.into();
    assert!(err.to_string().contains("server.port is 0"));
}

// ---- Lambda settings -----------------------------------------------------------

#[test]
fn concurrency_defaults_to_one() {
    assert_eq!(lambda_concurrency(|_: &str| None), 1);
    assert_eq!(lambda_concurrency(|_: &str| Some("junk".to_owned())), 1);
    assert_eq!(lambda_concurrency(|_: &str| Some("0".to_owned())), 1);
    // Same rule as lambda_runtime: no trim.
    assert_eq!(lambda_concurrency(|_: &str| Some(" 8 ".to_owned())), 1);
    assert_eq!(lambda_concurrency(|_: &str| Some("8".to_owned())), 8);
}

#[test]
fn shutdown_timing_warning_names_slow_settings() {
    let fast = ServerConfig {
        prestop_grace_secs: 0,
        shutdown_timeout_secs: 1,
        ..ServerConfig::default()
    };
    assert!(slow_shutdown_settings(&fast).is_empty());
    assert_eq!(
        slow_shutdown_settings(&ServerConfig::default()),
        ["server.prestop_grace_secs", "server.shutdown_timeout_secs"]
    );
}
