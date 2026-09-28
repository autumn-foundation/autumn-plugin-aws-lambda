//! The Autumn plugin.

use std::net::SocketAddr;
use std::time::Duration;

use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;

use crate::{Activation, ResponseMode};

/// Runs an Autumn app on AWS Lambda.
///
/// Register it with `.plugin(AwsLambdaPlugin::new())`.
#[derive(Debug, Clone)]
#[must_use]
pub struct AwsLambdaPlugin {
    activation: Activation,
    response_mode: ResponseMode,
    upstream: Option<SocketAddr>,
    readiness_timeout: Duration,
    timeout_margin: Duration,
}

impl Default for AwsLambdaPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl AwsLambdaPlugin {
    /// Default time to wait for Autumn startup.
    pub const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(10);
    /// Default time to keep free before the invocation deadline.
    pub const DEFAULT_TIMEOUT_MARGIN: Duration = Duration::from_millis(100);

    /// Makes the plugin with default settings.
    pub const fn new() -> Self {
        Self {
            activation: Activation::Auto,
            response_mode: ResponseMode::Buffered,
            upstream: None,
            readiness_timeout: Self::DEFAULT_READINESS_TIMEOUT,
            timeout_margin: Self::DEFAULT_TIMEOUT_MARGIN,
        }
    }

    /// Sets when the runtime loop starts. Default: [`Activation::Auto`].
    pub const fn activation(mut self, activation: Activation) -> Self {
        self.activation = activation;
        self
    }

    /// Sets how responses go back to Lambda. Default: [`ResponseMode::Buffered`].
    pub const fn response_mode(mut self, mode: ResponseMode) -> Self {
        self.response_mode = mode;
        self
    }

    /// Sets the upstream address. Default: from Autumn `[server]` config.
    pub const fn upstream(mut self, addr: SocketAddr) -> Self {
        self.upstream = Some(addr);
        self
    }

    /// Sets the maximum time to wait for Autumn startup before the first event.
    pub const fn readiness_timeout(mut self, timeout: Duration) -> Self {
        self.readiness_timeout = timeout;
        self
    }

    /// Sets the time to keep free before the invocation deadline.
    pub const fn timeout_margin(mut self, margin: Duration) -> Self {
        self.timeout_margin = margin;
        self
    }
}

impl Plugin for AwsLambdaPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        let _ = self;
        app
    }
}
