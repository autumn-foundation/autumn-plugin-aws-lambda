//! End-to-end tests: a real Autumn app (`examples/hello.rs`) in a child
//! process, and a fake Lambda Runtime API in this process.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use tokio::sync::{Mutex, mpsc};

const WAIT: Duration = Duration::from_secs(60);

/// One result that the runtime loop posted.
#[derive(Debug)]
struct Posted {
    request_id: String,
    kind: &'static str,
    headers: HeaderMap,
    body: Bytes,
}

#[derive(Clone)]
struct Fake {
    events: Arc<Mutex<mpsc::Receiver<(String, String)>>>,
    posted: mpsc::Sender<Posted>,
}

async fn next(State(fake): State<Fake>) -> impl IntoResponse {
    let (id, event) = fake.events.lock().await.recv().await.expect("event");
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis()
        + 30_000;
    (
        [
            ("lambda-runtime-aws-request-id", id),
            ("lambda-runtime-deadline-ms", deadline.to_string()),
            (
                "lambda-runtime-invoked-function-arn",
                "arn:aws:lambda:us-east-1:123:function:hello".to_owned(),
            ),
            ("content-type", "application/json".to_owned()),
        ],
        event,
    )
}

async fn response(
    State(fake): State<Fake>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    record(&fake, id, "response", headers, body).await
}

async fn error(
    State(fake): State<Fake>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    record(&fake, id, "error", headers, body).await
}

async fn record(
    fake: &Fake,
    request_id: String,
    kind: &'static str,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let _ = fake
        .posted
        .send(Posted {
            request_id,
            kind,
            headers,
            body,
        })
        .await;
    StatusCode::ACCEPTED
}

async fn spawn_fake() -> (
    SocketAddr,
    mpsc::Sender<(String, String)>,
    mpsc::Receiver<Posted>,
) {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (posted_tx, posted_rx) = mpsc::channel(8);
    let fake = Fake {
        events: Arc::new(Mutex::new(event_rx)),
        posted: posted_tx,
    };
    let app = Router::new()
        .route("/2018-06-01/runtime/invocation/next", get(next))
        .route(
            "/2018-06-01/runtime/invocation/{id}/response",
            post(response),
        )
        .route("/2018-06-01/runtime/invocation/{id}/error", post(error))
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await });
    (addr, event_tx, posted_rx)
}

/// Builds the example and returns its path.
fn example_binary() -> PathBuf {
    // current_exe: <target>/<profile>/deps/e2e-<hash>
    let exe = std::env::current_exe().expect("exe");
    let profile_dir = exe.parent().and_then(|p| p.parent()).expect("profile dir");
    let target_dir = profile_dir.parent().expect("target dir");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "--example", "hello", "--target-dir"])
        .arg(target_dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("cargo build");
    assert!(status.success(), "example build failed");
    profile_dir.join("examples").join("hello")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

/// Stops the child process on drop: SIGTERM first, then kill.
struct App(Child);

impl App {
    /// Sends SIGTERM, like Lambda at shutdown.
    fn terminate(&self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .stderr(Stdio::null())
            .status();
    }

    /// Waits for exit. Returns `None` on timeout.
    fn wait_exit(&mut self, limit: Duration) -> Option<std::process::ExitStatus> {
        let end = std::time::Instant::now() + limit;
        while std::time::Instant::now() < end {
            if let Ok(Some(status)) = self.0.try_wait() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        self.terminate();
        if self.wait_exit(Duration::from_secs(10)).is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Autumn checks the `Host` header. Lambda URLs must be trusted.
const TRUSTED_HOSTS: &str = "localhost,.lambda-url.us-east-1.on.aws";

fn spawn_app(runtime_api: Option<SocketAddr>, port: u16, mode: &str) -> App {
    spawn_app_with_hosts(runtime_api, port, mode, TRUSTED_HOSTS)
}

fn spawn_app_with_hosts(
    runtime_api: Option<SocketAddr>,
    port: u16,
    mode: &str,
    hosts: &str,
) -> App {
    let mut cmd = Command::new(example_binary());
    cmd.env("AUTUMN_SERVER__PORT", port.to_string())
        .env("AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS", hosts)
        .env("LAMBDA_RESPONSE_MODE", mode)
        .env("AWS_LAMBDA_FUNCTION_NAME", "hello")
        .env("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "128")
        .env("AWS_LAMBDA_FUNCTION_VERSION", "$LATEST")
        .env("AUTUMN_SERVER__PRESTOP_GRACE_SECS", "0")
        .env("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS", "2")
        .env_remove("AWS_LAMBDA_RUNTIME_API")
        .env_remove("AWS_LAMBDA_MAX_CONCURRENCY")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(api) = runtime_api {
        cmd.env("AWS_LAMBDA_RUNTIME_API", api.to_string());
    }
    App(cmd.spawn().expect("spawn example"))
}

fn event(path: &str) -> String {
    serde_json::json!({
        "version": "2.0",
        "routeKey": "$default",
        "rawPath": path,
        "rawQueryString": "",
        "headers": { "host": "fn.lambda-url.us-east-1.on.aws" },
        "requestContext": {
            "accountId": "123",
            "apiId": "fn",
            "domainName": "fn.lambda-url.us-east-1.on.aws",
            "domainPrefix": "fn",
            "http": {
                "method": "GET",
                "path": path,
                "protocol": "HTTP/1.1",
                "sourceIp": "203.0.113.9",
                "userAgent": "e2e"
            },
            "requestId": "url-id",
            "routeKey": "$default",
            "stage": "$default",
            "time": "28/Sep/2026:00:00:00 +0000",
            "timeEpoch": 0
        },
        "isBase64Encoded": false
    })
    .to_string()
}

async fn invoke(
    events: &mpsc::Sender<(String, String)>,
    posted: &mut mpsc::Receiver<Posted>,
    id: &str,
    path: &str,
) -> Posted {
    events
        .send((id.to_owned(), event(path)))
        .await
        .expect("send event");
    let got = tokio::time::timeout(WAIT, posted.recv())
        .await
        .expect("response in time")
        .expect("posted");
    assert_eq!(got.request_id, id);
    got
}

fn json_body(p: &Posted) -> serde_json::Value {
    serde_json::from_slice(&p.body).expect("json response")
}

#[tokio::test]
async fn buffered_invocations_reach_the_app() {
    let (api, events, mut posted) = spawn_fake().await;
    let _app = spawn_app(Some(api), free_port(), "buffered");

    let got = invoke(&events, &mut posted, "e2e-1", "/hello").await;
    assert_eq!(got.kind, "response");
    let v = json_body(&got);
    assert_eq!(v["statusCode"], 200, "{v}");
    assert_eq!(v["body"], "hello from autumn (request e2e-1)");

    // A second event uses the same loop and connection pool.
    let got = invoke(&events, &mut posted, "e2e-2", "/missing").await;
    assert_eq!(got.kind, "response");
    assert_eq!(json_body(&got)["statusCode"], 404);
}

#[tokio::test]
async fn streaming_invocation_reaches_the_app() {
    let (api, events, mut posted) = spawn_fake().await;
    let _app = spawn_app(Some(api), free_port(), "streaming");

    let got = invoke(&events, &mut posted, "e2e-s", "/hello").await;
    assert_eq!(got.kind, "response");
    assert_eq!(
        got.headers["lambda-runtime-function-response-mode"],
        "streaming"
    );
    // Streaming format: JSON prelude, 8 NUL bytes, then the body.
    let split = got
        .body
        .windows(8)
        .position(|w| w == [0u8; 8])
        .expect("prelude separator");
    let prelude: serde_json::Value =
        serde_json::from_slice(&got.body[..split]).expect("prelude json");
    assert_eq!(prelude["statusCode"], 200);
    assert_eq!(&got.body[split + 8..], b"hello from autumn (request e2e-s)");
}

#[tokio::test]
async fn untrusted_host_is_a_400_response_not_an_invocation_error() {
    let (api, events, mut posted) = spawn_fake().await;
    let _app = spawn_app_with_hosts(Some(api), free_port(), "buffered", "localhost");

    let got = invoke(&events, &mut posted, "e2e-h", "/hello").await;
    assert_eq!(got.kind, "response");
    let v = json_body(&got);
    assert_eq!(v["statusCode"], 400, "{v}");
    assert!(
        v["body"]
            .as_str()
            .is_some_and(|b| b.contains("Invalid Host header"))
    );
}

#[tokio::test]
async fn sigterm_stops_the_app_cleanly_while_the_loop_waits() {
    let (api, events, mut posted) = spawn_fake().await;
    let mut app = spawn_app(Some(api), free_port(), "buffered");

    // One event proves the loop runs. Then the loop waits on `/next`.
    let got = invoke(&events, &mut posted, "e2e-t", "/hello").await;
    assert_eq!(json_body(&got)["statusCode"], 200);

    app.terminate();
    let status = tokio::task::spawn_blocking(move || app.wait_exit(WAIT))
        .await
        .expect("join")
        .expect("exit in time");
    assert!(status.success(), "{status}");
}

#[tokio::test]
async fn runtime_api_failure_exits_with_code_1() {
    // A closed port: `/next` fails, so the loop stops.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr")
    };
    let mut app = spawn_app(Some(closed), free_port(), "buffered");
    let status = tokio::task::spawn_blocking(move || app.wait_exit(WAIT))
        .await
        .expect("join")
        .expect("exit in time");
    assert_eq!(status.code(), Some(1), "{status}");
}

#[tokio::test]
async fn app_serves_normally_outside_lambda() {
    let port = free_port();
    let _app = spawn_app(None, port, "buffered");
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Ok(mut s) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            s.write_all(b"GET /hello HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
                .await
                .expect("write");
            let mut out = String::new();
            s.read_to_string(&mut out).await.expect("read");
            assert!(out.starts_with("HTTP/1.1 200"), "{out}");
            assert!(out.contains("hello from autumn"), "{out}");
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "app did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
