//! The Lambda runtime loop and the readiness wait.

use std::time::Duration;

use crate::LambdaProxy;

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
/// The delay starts at `initial` and doubles up to `max`.
/// Returns the last value of `ready`.
pub async fn wait_until(
    ready: impl Fn() -> bool,
    timeout: Duration,
    initial: Duration,
    max: Duration,
) -> bool {
    let _ = (ready, timeout, initial, max);
    todo!()
}

/// Runs the Lambda runtime loop with `proxy`. Returns only on a fatal error.
pub(crate) async fn run(proxy: LambdaProxy, mode: ResponseMode) -> Result<(), lambda_http::Error> {
    let _ = (proxy, mode);
    todo!()
}
