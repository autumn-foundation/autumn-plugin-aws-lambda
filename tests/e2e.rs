//! End-to-end tests: a real Autumn app (`examples/hello.rs`) in a child
//! process, and a fake Lambda Runtime API in this process.

use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
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

/// Returns `(target dir, profile dir)` of this test binary.
fn dirs() -> (PathBuf, PathBuf) {
    // current_exe: <target>/<profile>/deps/e2e-<hash>
    let exe = std::env::current_exe().expect("exe");
    let profile_dir = exe
        .parent()
        .and_then(|p| p.parent())
        .expect("profile dir")
        .to_path_buf();
    let target_dir = profile_dir.parent().expect("target dir").to_path_buf();
    (target_dir, profile_dir)
}

/// Builds the example one time, with the profile of this test binary.
fn example_binary() -> &'static FsPath {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let (target_dir, profile_dir) = dirs();
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["build", "--quiet", "--example", "hello", "--target-dir"])
            .arg(&target_dir)
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            cmd.arg("--release");
        }
        assert!(
            cmd.status().expect("cargo build").success(),
            "example build failed"
        );
        profile_dir.join("examples").join("hello")
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

/// A child app process. Drop stops it: SIGTERM first, then kill.
struct App {
    child: Child,
    log: PathBuf,
}

impl App {
    /// Sends SIGTERM, like Lambda at shutdown.
    fn terminate(&self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .stderr(Stdio::null())
            .status();
    }

    /// Waits for exit. Returns `None` on timeout.
    fn wait_exit(&mut self, limit: Duration) -> Option<std::process::ExitStatus> {
        let end = std::time::Instant::now() + limit;
        while std::time::Instant::now() < end {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// Returns the output of the app (stdout and stderr).
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        self.terminate();
        if self.wait_exit(Duration::from_secs(10)).is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Autumn checks the `Host` header. Lambda URLs must be trusted.
const TRUSTED_HOSTS: &str = "localhost,.lambda-url.us-east-1.on.aws";

/// Options for one child app.
struct Opts<'a> {
    runtime_api: Option<SocketAddr>,
    mode: &'a str,
    hosts: &'a str,
    concurrency: Option<u32>,
}

impl Default for Opts<'_> {
    fn default() -> Self {
        Self {
            runtime_api: None,
            mode: "buffered",
            hosts: TRUSTED_HOSTS,
            concurrency: None,
        }
    }
}

fn spawn_app(runtime_api: Option<SocketAddr>, port: u16, mode: &str) -> App {
    spawn_with(
        port,
        &Opts {
            runtime_api,
            mode,
            ..Opts::default()
        },
    )
}

/// Starts the example with a clean environment. Output goes to a log file.
fn spawn_with(port: u16, opts: &Opts<'_>) -> App {
    let (target_dir, _) = dirs();
    let log_dir = target_dir.join("e2e-logs");
    std::fs::create_dir_all(&log_dir).expect("log dir");
    let log = log_dir.join(format!("app-{port}.log"));
    let out = std::fs::File::create(&log).expect("log file");
    let err = out.try_clone().expect("log file");

    let mut cmd = Command::new(example_binary());
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("AUTUMN_LOG__LEVEL", "error")
        .env("AUTUMN_SERVER__PORT", port.to_string())
        .env("AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS", opts.hosts)
        .env("AUTUMN_SERVER__PRESTOP_GRACE_SECS", "0")
        .env("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS", "1")
        .env("LAMBDA_RESPONSE_MODE", opts.mode)
        .env("AWS_LAMBDA_FUNCTION_NAME", "hello")
        .env("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "128")
        .env("AWS_LAMBDA_FUNCTION_VERSION", "$LATEST")
        .current_dir(&log_dir)
        .stdout(out)
        .stderr(err);
    // Keep coverage data from the child under `cargo llvm-cov`.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", profile);
    }
    if let Some(api) = opts.runtime_api {
        cmd.env("AWS_LAMBDA_RUNTIME_API", api.to_string());
    }
    if let Some(n) = opts.concurrency {
        cmd.env("AWS_LAMBDA_MAX_CONCURRENCY", n.to_string());
    }
    App {
        child: cmd.spawn().expect("spawn example"),
        log,
    }
}

/// Returns an address with no listener: `/next` fails there.
fn closed_addr() -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    l.local_addr().expect("addr")
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
    let _app = spawn_with(
        free_port(),
        &Opts {
            runtime_api: Some(api),
            hosts: "localhost",
            ..Opts::default()
        },
    );

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
    let mut app = spawn_app(Some(closed_addr()), free_port(), "buffered");
    let status = tokio::task::spawn_blocking(move || {
        let status = app.wait_exit(WAIT);
        (status, app.log())
    })
    .await
    .expect("join");
    let (status, log) = status;
    assert_eq!(status.and_then(|s| s.code()), Some(1), "{log}");
    assert!(log.contains("AWS Lambda runtime loop stopped"), "{log}");
}

#[tokio::test]
async fn concurrent_mode_serves_events() {
    let (api, events, mut posted) = spawn_fake().await;
    let _app = spawn_with(
        free_port(),
        &Opts {
            runtime_api: Some(api),
            concurrency: Some(2),
            ..Opts::default()
        },
    );
    for id in ["c-1", "c-2", "c-3"] {
        let got = invoke(&events, &mut posted, id, "/hello").await;
        assert_eq!(
            json_body(&got)["body"],
            format!("hello from autumn (request {id})")
        );
    }
}

#[tokio::test]
async fn concurrent_mode_exits_when_the_runtime_api_is_gone() {
    // lambda_runtime retries `/next` with no end in concurrent mode.
    // The plugin watchdog must stop the process.
    let mut app = spawn_with(
        free_port(),
        &Opts {
            runtime_api: Some(closed_addr()),
            concurrency: Some(4),
            ..Opts::default()
        },
    );
    let (status, log) = tokio::task::spawn_blocking(move || {
        let status = app.wait_exit(WAIT);
        (status, app.log())
    })
    .await
    .expect("join");
    let tail: String = log
        .chars()
        .rev()
        .take(2000)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    assert_eq!(status.and_then(|s| s.code()), Some(1), "{tail}");
    assert!(log.contains("Runtime API is not reachable"), "{tail}");
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
