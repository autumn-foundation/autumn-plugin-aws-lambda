//! The Lambda runtime loop and the readiness wait.

use std::time::Duration;

use crate::PluginError;
use crate::proxy::LambdaProxy;
use crate::timing::{as_ms, next_delay_ms};

/// First delay of the readiness wait.
const READY_INITIAL: Duration = Duration::from_millis(5);
/// Longest delay of the readiness wait.
const READY_MAX: Duration = Duration::from_millis(100);
/// Name of the runtime loop thread.
const THREAD_NAME: &str = "aws-lambda-runtime";
/// Time between two Runtime API checks in concurrent mode.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// Failed Runtime API checks in sequence before the process exits.
const WATCH_LIMIT: u32 = 5;

/// How the plugin sends responses to Lambda.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseMode {
    /// Read the full body (up to 6 MiB), then send it. It works with all
    /// HTTP event sources. This is the default.
    #[default]
    Buffered,
    /// Stream the body. Use it only with a Function URL or with API
    /// Gateway response streaming.
    Streaming,
}

/// The state of Autumn startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Startup {
    /// Startup hooks still run.
    Pending,
    /// Autumn is ready.
    Complete,
    /// Autumn shuts down.
    Stopping,
}

/// Settings for the runtime loop thread.
#[derive(Debug)]
pub(crate) struct LoopSettings {
    pub proxy: LambdaProxy,
    pub mode: ResponseMode,
    /// `None`: wait until startup is complete, with no time limit.
    pub readiness_timeout: Option<Duration>,
    /// Events to handle at the same time.
    pub concurrency: u32,
    /// Address of the Runtime API (`host:port`).
    pub runtime_api: String,
}

/// Calls `ready` until it returns `true` or until `timeout` ends.
///
/// The delay starts at `initial` (minimum 1 ms) and doubles up to `max`.
/// The last sleep stops at `timeout`. Returns the last value of `ready`.
/// Verus twin: `verification/core.rs::readiness_probes`.
pub async fn wait_until(
    ready: impl Fn() -> bool,
    timeout: Duration,
    initial: Duration,
    max: Duration,
) -> bool {
    let timeout_ms = as_ms(timeout);
    let mut delay_ms = as_ms(initial).max(1);
    let max_ms = as_ms(max).max(delay_ms);
    let start = tokio::time::Instant::now();
    loop {
        if ready() {
            return true;
        }
        let slept_ms = as_ms(start.elapsed());
        if slept_ms >= timeout_ms {
            return false;
        }
        let step = delay_ms.min(timeout_ms - slept_ms);
        tokio::time::sleep(Duration::from_millis(step)).await;
        delay_ms = next_delay_ms(delay_ms, max_ms);
    }
}

/// Starts the runtime loop on a new thread.
///
/// The thread waits for Autumn startup. If Autumn shuts down first, the
/// loop does not start. The `lambda_http` loop future is not `Send`, so
/// `tokio::spawn` cannot accept it. The loop gets its own thread and
/// runtime.
///
/// The loop runs until the process ends. If it stops or panics, the
/// process exits with code 1. Lambda then starts a new execution
/// environment. This exit does not run Autumn shutdown hooks.
///
/// # Errors
///
/// Returns an error if the thread cannot start.
pub(crate) fn spawn_loop(
    settings: LoopSettings,
    startup: impl Fn() -> Startup + Send + 'static,
) -> Result<(), PluginError> {
    std::thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || {
            let runtime = match build_runtime(settings.concurrency) {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(%error, "cannot build the Lambda runtime; exiting");
                    std::process::exit(1);
                }
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(run_when_ready(settings, startup))
            }));
            if matches!(outcome, Ok(None)) {
                return;
            }
            tracing::error!(?outcome, "AWS Lambda runtime loop stopped; exiting");
            std::process::exit(1);
        })
        .map_err(PluginError::Runtime)?;
    Ok(())
}

/// Waits for startup, then runs the loop. Returns `None` if the loop does
/// not start.
#[allow(
    clippy::future_not_send,
    reason = "the lambda_http loop is not Send; it runs on its own thread"
)]
async fn run_when_ready(
    settings: LoopSettings,
    startup: impl Fn() -> Startup,
) -> Option<Result<(), lambda_http::Error>> {
    let limit = settings.readiness_timeout.unwrap_or(Duration::MAX);
    wait_until(
        || startup() != Startup::Pending,
        limit,
        READY_INITIAL,
        READY_MAX,
    )
    .await;
    match startup() {
        Startup::Stopping => {
            tracing::info!("Autumn shuts down; the Lambda loop does not start");
            return None;
        }
        Startup::Pending => tracing::warn!(
            ?limit,
            "Autumn startup is not complete; the Lambda loop starts now"
        ),
        Startup::Complete => {}
    }
    if settings.concurrency > 1 {
        // In concurrent mode, lambda_runtime retries a failed `/next` with
        // no delay and does not stop. This check stops the process instead.
        tokio::spawn(watch_runtime_api(settings.runtime_api));
    }
    tracing::info!(mode = ?settings.mode, concurrency = settings.concurrency, "AWS Lambda runtime loop runs");
    Some(run(settings.proxy, settings.mode).await)
}

/// Builds the loop runtime. One thread is enough for one event at a time.
fn build_runtime(concurrency: u32) -> std::io::Result<tokio::runtime::Runtime> {
    if concurrency <= 1 {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
    }
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let wanted = usize::try_from(concurrency).unwrap_or(usize::MAX);
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cores.min(wanted))
        .enable_all()
        .build()
}

/// Exits the process when the Runtime API refuses connections
/// [`WATCH_LIMIT`] times in sequence.
async fn watch_runtime_api(address: String) {
    let mut failures = 0;
    loop {
        tokio::time::sleep(WATCH_INTERVAL).await;
        let connect = tokio::net::TcpStream::connect(address.as_str());
        match tokio::time::timeout(WATCH_INTERVAL, connect).await {
            Ok(Ok(_)) => failures = 0,
            _ => failures += 1,
        }
        if failures >= WATCH_LIMIT {
            tracing::error!(%address, "AWS Lambda Runtime API is not reachable; exiting");
            std::process::exit(1);
        }
    }
}

/// Runs the Lambda runtime loop with `proxy`. It returns only on a fatal error.
///
/// `lambda_http` uses concurrent mode when `AWS_LAMBDA_MAX_CONCURRENCY` is
/// more than 1. Otherwise, it handles one event at a time.
#[allow(
    clippy::future_not_send,
    reason = "the lambda_http loop is not Send; it runs on its own thread"
)]
async fn run(proxy: LambdaProxy, mode: ResponseMode) -> Result<(), lambda_http::Error> {
    let proxy = proxy.response_mode(mode);
    match mode {
        ResponseMode::Streaming => lambda_http::run_with_streaming_response_concurrent(proxy).await,
        _ => lambda_http::run_concurrent(proxy).await,
    }
}
