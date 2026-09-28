//! The Lambda runtime loop and the readiness wait.

use std::time::Duration;

use crate::timing::{as_ms, next_delay_ms};
use crate::{LambdaProxy, PluginError};

/// First delay of the readiness wait.
const READY_INITIAL: Duration = Duration::from_millis(5);
/// Longest delay of the readiness wait.
const READY_MAX: Duration = Duration::from_millis(100);
/// Name of the runtime loop thread.
const THREAD_NAME: &str = "aws-lambda-runtime";

/// How the plugin returns responses to Lambda.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResponseMode {
    /// Read the full body, then return it. Works with all HTTP event
    /// sources. This is the default.
    #[default]
    Buffered,
    /// Stream the body. Needs a Function URL or API Gateway with
    /// response streaming.
    Streaming,
}

/// Calls `ready` until it returns `true` or `timeout` passes.
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

/// Runs the Lambda runtime loop with `proxy`. Returns only on a fatal error.
///
/// Uses concurrent mode when `AWS_LAMBDA_MAX_CONCURRENCY` is above 1
/// (Lambda Managed Instances). Otherwise it handles one event at a time.
pub async fn run(proxy: LambdaProxy, mode: ResponseMode) -> Result<(), lambda_http::Error> {
    match mode {
        ResponseMode::Buffered => lambda_http::run_concurrent(proxy.buffer_body(true)).await,
        ResponseMode::Streaming => {
            lambda_http::run_with_streaming_response_concurrent(proxy.buffer_body(false)).await
        }
    }
}

/// Starts the runtime loop on a new thread.
///
/// The thread waits for `is_ready` (up to `readiness_timeout`), then runs
/// the loop. The `lambda_http` loop future is not `Send`, so it cannot go
/// to `tokio::spawn`. It gets its own thread and current-thread runtime.
///
/// The loop runs forever. If it stops or panics, the process exits with
/// code 1, and Lambda starts a new execution environment.
///
/// # Errors
///
/// Returns an error when the runtime or the thread cannot start.
pub fn spawn_loop(
    proxy: LambdaProxy,
    mode: ResponseMode,
    readiness_timeout: Duration,
    is_ready: impl Fn() -> bool + Send + 'static,
) -> Result<(), PluginError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(PluginError::Runtime)?;
    std::thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(async move {
                    if !wait_until(is_ready, readiness_timeout, READY_INITIAL, READY_MAX).await {
                        tracing::warn!(
                            ?readiness_timeout,
                            "Autumn startup not complete; starting Lambda loop anyway"
                        );
                    }
                    tracing::info!(?mode, "starting AWS Lambda runtime loop");
                    run(proxy, mode).await
                })
            }));
            tracing::error!(?outcome, "AWS Lambda runtime loop stopped; exiting");
            std::process::exit(1);
        })
        .map_err(PluginError::Runtime)?;
    Ok(())
}
