//! Integration tests: `LambdaProxy` against a real local HTTP server.

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use autumn_plugin_aws_lambda::__private::{LambdaProxy, ProxyBody, Upstream};
use autumn_plugin_aws_lambda::ResponseMode;
use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use http_body_util::BodyExt;
use lambda_http::RequestExt;
use lambda_http::lambda_runtime::Context;
use tower_service::Service;

/// Routes that show what the request looked like upstream.
fn request_routes() -> Router {
    Router::new()
        .route(
            "/echo",
            post(|headers: HeaderMap, body: Bytes| async move {
                let ct = headers
                    .get("content-type")
                    .map(|v| v.to_str().unwrap_or_default().to_owned())
                    .unwrap_or_default();
                ([("content-type", ct)], body)
            }),
        )
        .route(
            "/headers",
            get(|headers: HeaderMap| async move {
                let pick = |n: &str| {
                    headers
                        .get(n)
                        .map_or("-", |v| v.to_str().unwrap_or("?"))
                        .to_owned()
                };
                format!(
                    "host={} rid={} conn={} cl={} xff={} xfh={}",
                    pick("host"),
                    pick("lambda-runtime-aws-request-id"),
                    pick("x-hop"),
                    pick("content-length"),
                    pick("x-forwarded-for"),
                    pick("x-forwarded-host")
                )
            }),
        )
        .route(
            "/query",
            get(|uri: axum::http::Uri| async move { uri.to_string() }),
        )
        .route(
            "/cookie-in",
            get(|headers: HeaderMap| async move {
                headers
                    .get("cookie")
                    .map_or("-", |v| v.to_str().unwrap_or("?"))
                    .to_owned()
            }),
        )
}

/// Routes that return special responses.
fn response_routes() -> Router {
    Router::new()
        .route(
            "/cookies",
            get(|| async {
                (
                    axum::response::AppendHeaders([
                        ("set-cookie", "a=1"),
                        ("set-cookie", "b=2"),
                        ("connection", "x-internal"),
                        ("x-internal", "secret"),
                    ]),
                    axum::body::Body::from_stream(futures_util::stream::iter([Ok::<
                        _,
                        std::io::Error,
                    >(
                        Bytes::from_static(b"ok"),
                    )])),
                )
            }),
        )
        .route(
            "/vary",
            get(|| async {
                (
                    axum::response::AppendHeaders([
                        ("vary", "origin"),
                        ("vary", "accept-encoding"),
                    ]),
                    "ok",
                )
            }),
        )
        .route(
            "/latin1",
            get(|| async {
                (
                    [("content-type", "text/csv; charset=windows-1252")],
                    Bytes::from_static(b"caf\xe9"),
                )
            }),
        )
        .route(
            "/big",
            get(|| async { Bytes::from(vec![b'x'; 7 * 1024 * 1024]) }),
        )
        .fallback(|uri: axum::http::Uri| async move { format!("fallback {uri}") })
        .route(
            "/status",
            get(|| async { (StatusCode::IM_A_TEAPOT, "short and stout") }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "late"
            }),
        )
        .route(
            "/slow-body",
            get(|| async {
                let stream = async_stream(Duration::from_secs(5));
                axum::body::Body::from_stream(stream)
            }),
        )
}

async fn spawn_upstream() -> SocketAddr {
    let app = request_routes().merge(response_routes());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await });
    addr
}

/// A body that sends one chunk, then waits `delay`, then ends.
fn async_stream(
    delay: Duration,
) -> impl futures_core::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    futures_util::stream::unfold(0u8, move |step| async move {
        match step {
            0 => Some((Ok(Bytes::from_static(b"first ")), 1)),
            1 => {
                tokio::time::sleep(delay).await;
                Some((Ok(Bytes::from_static(b"second")), 2))
            }
            _ => None,
        }
    })
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("fits u64")
}

fn ctx(request_id: &str, deadline_in: Duration) -> Context {
    let mut ctx = Context::default();
    request_id.clone_into(&mut ctx.request_id);
    ctx.deadline = now_ms() + u64::try_from(deadline_in.as_millis()).expect("fits u64");
    ctx
}

/// Parses an API Gateway HTTP API (v2) event into a Lambda request.
fn apigw_v2(
    method: &str,
    path: &str,
    query: &str,
    body: Option<(&str, bool)>,
) -> lambda_http::Request {
    let (body, b64) = body.unwrap_or(("", false));
    let event = serde_json::json!({
        "version": "2.0",
        "routeKey": "$default",
        "rawPath": path,
        "rawQueryString": query,
        "headers": {
            "host": "abc.execute-api.us-east-1.amazonaws.com",
            "content-type": "application/octet-stream",
            "content-length": "999",
            "connection": "x-hop, host",
            "x-hop": "kept-on-requests",
            "x-forwarded-for": "10.9.9.9, 198.51.100.1",
            "x-forwarded-host": "victim.example.com"
        },
        "requestContext": {
            "accountId": "123",
            "apiId": "abc",
            "domainName": "abc.execute-api.us-east-1.amazonaws.com",
            "domainPrefix": "abc",
            "http": {
                "method": method,
                "path": path,
                "protocol": "HTTP/1.1",
                "sourceIp": "203.0.113.9",
                "userAgent": "test"
            },
            "requestId": "apigw-id",
            "routeKey": "$default",
            "stage": "$default",
            "time": "28/Sep/2026:00:00:00 +0000",
            "timeEpoch": 0
        },
        "body": body,
        "isBase64Encoded": b64
    });
    lambda_http::request::from_str(&event.to_string()).expect("valid event")
}

async fn call(
    proxy: &mut LambdaProxy,
    req: lambda_http::Request,
) -> (StatusCode, HeaderMap, Bytes) {
    let resp: http::Response<ProxyBody> = proxy.call(req).await.expect("infallible");
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.expect("body").to_bytes();
    (parts.status, parts.headers, bytes)
}

fn proxy(addr: SocketAddr) -> LambdaProxy {
    LambdaProxy::new(Upstream::new(addr))
}

#[tokio::test]
async fn get_with_query_reaches_upstream() {
    let addr = spawn_upstream().await;
    let (status, _, body) = call(
        &mut proxy(addr),
        apigw_v2("GET", "/query", "a=1&b=two", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "/query?a=1&b=two");
}

#[tokio::test]
async fn text_body_round_trips() {
    let addr = spawn_upstream().await;
    let (status, headers, body) = call(
        &mut proxy(addr),
        apigw_v2("POST", "/echo", "", Some(("hello lambda", false))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(body, "hello lambda");
}

#[tokio::test]
async fn base64_binary_body_round_trips() {
    let addr = spawn_upstream().await;
    // "AAEC/w==" is [0x00, 0x01, 0x02, 0xff].
    let (status, _, body) = call(
        &mut proxy(addr),
        apigw_v2("POST", "/echo", "", Some(("AAEC/w==", true))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), [0x00, 0x01, 0x02, 0xff]);
}

#[tokio::test]
async fn request_headers_are_prepared() {
    let addr = spawn_upstream().await;
    let req = apigw_v2("GET", "/headers", "", None)
        .with_lambda_context(ctx("lambda-req-7", Duration::from_secs(30)));
    let (_, _, body) = call(&mut proxy(addr), req).await;
    // Connection tokens on the event do not remove headers. The client IP
    // comes from the request context. Client forwarding headers are dropped.
    assert_eq!(
        body,
        "host=abc.execute-api.us-east-1.amazonaws.com rid=lambda-req-7 conn=kept-on-requests \
         cl=- xff=203.0.113.9 xfh=-"
    );
}

#[tokio::test]
async fn response_hop_by_hop_headers_are_removed() {
    let addr = spawn_upstream().await;
    let (_, headers, body) = call(&mut proxy(addr), apigw_v2("GET", "/cookies", "", None)).await;
    assert_eq!(body, "ok");
    let cookies: Vec<_> = headers.get_all("set-cookie").iter().collect();
    assert_eq!(cookies, ["a=1", "b=2"]);
    assert!(headers.get("connection").is_none());
    assert!(headers.get("x-internal").is_none());
    assert!(headers.get("transfer-encoding").is_none());
}

#[tokio::test]
async fn upstream_status_passes_through() {
    let addr = spawn_upstream().await;
    let (status, _, body) = call(&mut proxy(addr), apigw_v2("GET", "/status", "", None)).await;
    assert_eq!(status, StatusCode::IM_A_TEAPOT);
    assert_eq!(body, "short and stout");
}

#[tokio::test]
async fn closed_upstream_gives_502() {
    // Bind, then drop, so the port is closed.
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr")
    };
    let (status, _, body) = call(&mut proxy(addr), apigw_v2("GET", "/", "", None)).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body, "Bad Gateway");
}

#[tokio::test]
async fn slow_upstream_past_deadline_gives_504() {
    let addr = spawn_upstream().await;
    let req = apigw_v2("GET", "/slow", "", None)
        .with_lambda_context(ctx("r", Duration::from_millis(300)));
    let mut p = proxy(addr).timeout_margin(Duration::from_millis(50));
    let started = std::time::Instant::now();
    let (status, _, body) = call(&mut p, req).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(body, "Gateway Timeout");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn slow_body_past_deadline_gives_504_when_buffered() {
    let addr = spawn_upstream().await;
    let req = apigw_v2("GET", "/slow-body", "", None)
        .with_lambda_context(ctx("r", Duration::from_millis(300)));
    let (status, _, _) = call(&mut proxy(addr), req).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
}

#[tokio::test]
async fn streaming_returns_head_then_streams_body() {
    let addr = spawn_upstream().await;
    let req = apigw_v2("GET", "/slow-body", "", None)
        .with_lambda_context(ctx("r", Duration::from_secs(30)));
    let mut p = proxy(addr).response_mode(ResponseMode::Streaming);
    let resp = tokio::time::timeout(Duration::from_secs(2), p.call(req))
        .await
        .expect("head before body ends")
        .expect("infallible");
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body();
    let first = body.frame().await.expect("frame").expect("ok");
    assert_eq!(first.into_data().expect("data"), "first ");
}

#[tokio::test]
async fn request_without_lambda_context_has_no_request_id_or_deadline() {
    let addr = spawn_upstream().await;
    // A slow route still answers: no deadline applies.
    let (status, _, body) = call(&mut proxy(addr), apigw_v2("GET", "/headers", "", None)).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains(" rid=- "), "{body}");
}

#[tokio::test]
async fn proxy_is_reusable_across_calls() {
    let addr = spawn_upstream().await;
    let mut p = proxy(addr);
    for i in 0..5 {
        let q = format!("i={i}");
        let (status, _, body) = call(&mut p, apigw_v2("GET", "/query", &q, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("/query?{q}"));
    }
}

#[tokio::test]
async fn repeated_response_headers_are_folded() {
    let addr = spawn_upstream().await;
    let (_, headers, _) = call(&mut proxy(addr), apigw_v2("GET", "/vary", "", None)).await;
    let vary: Vec<_> = headers.get_all("vary").iter().collect();
    assert_eq!(vary, ["origin, accept-encoding"]);
}

#[tokio::test]
async fn buffered_response_has_no_stale_content_length() {
    let addr = spawn_upstream().await;
    let (_, headers, _) = call(&mut proxy(addr), apigw_v2("GET", "/status", "", None)).await;
    assert!(headers.get("content-length").is_none());
}

#[tokio::test]
async fn non_utf8_text_is_marked_binary() {
    let addr = spawn_upstream().await;
    let (_, headers, body) = call(&mut proxy(addr), apigw_v2("GET", "/latin1", "", None)).await;
    assert_eq!(headers["content-encoding"], "identity");
    assert_eq!(body.as_ref(), b"caf\xe9");
}

#[tokio::test]
async fn buffered_body_over_the_lambda_limit_gives_502() {
    let addr = spawn_upstream().await;
    let (status, _, _) = call(&mut proxy(addr), apigw_v2("GET", "/big", "", None)).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn streaming_has_no_size_limit() {
    let addr = spawn_upstream().await;
    let (status, _, body) = call(
        &mut proxy(addr).response_mode(ResponseMode::Streaming),
        apigw_v2("GET", "/big", "", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 7 * 1024 * 1024);
}

#[tokio::test]
async fn encoded_dot_segments_are_not_normalized() {
    let addr = spawn_upstream().await;
    let (_, _, body) = call(
        &mut proxy(addr),
        apigw_v2("GET", "/public/%2e%2e/query", "", None),
    )
    .await;
    assert_eq!(body, "fallback /public/%2e%2e/query");
}

/// Parses an API Gateway REST (v1) event with stage `prod`.
fn apigw_v1(path: &str) -> lambda_http::Request {
    let event = serde_json::json!({
        "resource": "/{proxy+}",
        "path": path,
        "httpMethod": "GET",
        "headers": { "host": "abc.execute-api.us-east-1.amazonaws.com" },
        "multiValueHeaders": { "host": ["abc.execute-api.us-east-1.amazonaws.com"] },
        "queryStringParameters": { "a": "1" },
        "multiValueQueryStringParameters": { "a": ["1"] },
        "requestContext": {
            "accountId": "123",
            "resourceId": "r",
            "stage": "prod",
            "requestId": "rest-id",
            "identity": { "sourceIp": "198.51.100.7" },
            "resourcePath": "/{proxy+}",
            "httpMethod": "GET",
            "apiId": "abc",
            "path": format!("/prod{path}")
        },
        "body": null,
        "isBase64Encoded": false
    });
    lambda_http::request::from_str(&event.to_string()).expect("valid v1 event")
}

#[tokio::test]
async fn rest_api_stage_is_not_in_the_upstream_path() {
    let addr = spawn_upstream().await;
    let (status, _, body) = call(&mut proxy(addr), apigw_v1("/query")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "/query?a=1");
}

#[tokio::test]
async fn rest_api_source_ip_becomes_x_forwarded_for() {
    let addr = spawn_upstream().await;
    let (_, _, body) = call(&mut proxy(addr), apigw_v1("/headers")).await;
    let body = String::from_utf8_lossy(&body);
    assert!(body.ends_with("xff=198.51.100.7 xfh=-"), "{body}");
}

#[tokio::test]
async fn timeout_margin_is_kept_free() {
    let addr = spawn_upstream().await;
    // A fast route. The deadline is 1 s away, but the margin is 2 s.
    let req =
        apigw_v2("GET", "/status", "", None).with_lambda_context(ctx("r", Duration::from_secs(1)));
    let (status, _, _) = call(&mut proxy(addr).timeout_margin(Duration::from_secs(2)), req).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
}

/// An upstream that sends a head that promises 100 bytes, sends 5, and closes.
async fn spawn_truncating_upstream() -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nshort")
                .await;
        }
    });
    addr
}

#[tokio::test]
async fn truncated_upstream_body_gives_502_when_buffered() {
    let addr = spawn_truncating_upstream().await;
    let (status, _, body) = call(&mut proxy(addr), apigw_v2("GET", "/", "", None)).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body, "Bad Gateway");
}

#[tokio::test]
async fn truncated_upstream_body_ends_the_stream_with_an_error() {
    let addr = spawn_truncating_upstream().await;
    let mut p = proxy(addr).response_mode(ResponseMode::Streaming);
    let resp = p
        .call(apigw_v2("GET", "/", "", None))
        .await
        .expect("infallible");
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.into_body().collect().await.is_err());
}

#[tokio::test]
async fn v2_cookies_reach_the_app_as_one_cookie_header() {
    let addr = spawn_upstream().await;
    let mut event: serde_json::Value = serde_json::from_str(
        r#"{"version":"2.0","routeKey":"$default","rawPath":"/cookie-in","rawQueryString":"",
            "cookies":["a=1","b=2"],"headers":{"host":"h"},
            "requestContext":{"http":{"method":"GET","path":"/cookie-in","protocol":"HTTP/1.1",
            "sourceIp":"203.0.113.9","userAgent":"t"},"stage":"$default","timeEpoch":0}}"#,
    )
    .expect("json");
    event["isBase64Encoded"] = serde_json::Value::Bool(false);
    let req = lambda_http::request::from_str(&event.to_string()).expect("event");
    let (_, _, body) = call(&mut proxy(addr), req).await;
    assert_eq!(body, "a=1; b=2");
}

#[tokio::test]
async fn alb_event_with_multi_value_query_reaches_the_app() {
    let addr = spawn_upstream().await;
    let event = serde_json::json!({
        "requestContext": { "elb": { "targetGroupArn": "arn:aws:elasticloadbalancing:tg" } },
        "httpMethod": "GET",
        "path": "/query",
        "multiValueQueryStringParameters": { "a": ["1", "2"] },
        "multiValueHeaders": {
            "host": ["alb.example.com"],
            "x-forwarded-for": ["10.0.0.1, 198.51.100.4"]
        },
        "body": "",
        "isBase64Encoded": false
    });
    let req = lambda_http::request::from_str(&event.to_string()).expect("alb event");
    let (status, _, body) = call(&mut proxy(addr), req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "/query?a=1&a=2");
}

#[tokio::test]
async fn alb_client_ip_is_the_last_forwarded_for_entry() {
    let addr = spawn_upstream().await;
    let event = serde_json::json!({
        "requestContext": { "elb": { "targetGroupArn": "arn:aws:elasticloadbalancing:tg" } },
        "httpMethod": "GET",
        "path": "/headers",
        "headers": { "host": "alb.example.com", "x-forwarded-for": "10.0.0.1, 198.51.100.4" },
        "body": "",
        "isBase64Encoded": false
    });
    let req = lambda_http::request::from_str(&event.to_string()).expect("alb event");
    let (_, _, body) = call(&mut proxy(addr), req).await;
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("xff=198.51.100.4 "), "{body}");
}

#[tokio::test]
async fn head_request_has_an_empty_body() {
    let addr = spawn_upstream().await;
    let (status, _, body) = call(&mut proxy(addr), apigw_v2("HEAD", "/status", "", None)).await;
    assert_eq!(status, StatusCode::IM_A_TEAPOT);
    assert!(body.is_empty());
}
