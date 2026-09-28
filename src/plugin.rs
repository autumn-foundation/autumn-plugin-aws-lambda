//! The Autumn plugin.

use std::net::SocketAddr;
use std::time::Duration;

use autumn_web::AppState;
use autumn_web::app::AppBuilder;
use autumn_web::config::ServerConfig;
use autumn_web::plugin::Plugin;

use crate::activation::{RUNTIME_API_ENV, lambda_concurrency, missing_lambda_env};
use crate::proxy::LambdaProxy;
use crate::runtime::{LoopSettings, Startup, spawn_loop};
use crate::upstream::Upstream;
use crate::{Activation, PluginError, ResponseMode};

/// Autumn sets this variable to run one task instead of the server.
const TASK_ENV: &str = "AUTUMN_RUN_TASK";

/// Runs an Autumn app on AWS Lambda.
///
/// Register it with `.plugin(AwsLambdaPlugin::new())`.
#[derive(Debug, Clone)]
pub struct AwsLambdaPlugin {
    activation: Activation,
    response_mode: ResponseMode,
    upstream: Option<SocketAddr>,
    readiness_timeout: Option<Duration>,
    timeout_margin: Duration,
}

impl Default for AwsLambdaPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl AwsLambdaPlugin {
    /// Default time between the end of the upstream call and the
    /// invocation deadline.
    pub const DEFAULT_TIMEOUT_MARGIN: Duration = Duration::from_millis(100);

    /// Makes the plugin with default settings.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            activation: Activation::Auto,
            response_mode: ResponseMode::Buffered,
            upstream: None,
            readiness_timeout: None,
            timeout_margin: Self::DEFAULT_TIMEOUT_MARGIN,
        }
    }

    /// Sets when the runtime loop starts. Default: [`Activation::Auto`].
    #[must_use]
    pub const fn activation(mut self, activation: Activation) -> Self {
        self.activation = activation;
        self
    }

    /// Sets how the plugin sends responses to Lambda.
    /// Default: [`ResponseMode::Buffered`].
    #[must_use]
    pub const fn response_mode(mut self, mode: ResponseMode) -> Self {
        self.response_mode = mode;
        self
    }

    /// Sets the upstream address. Default: from Autumn `[server]` config.
    ///
    /// With this setting, the plugin does not check `[server]`. The address
    /// must reach the Autumn listener.
    #[must_use]
    pub const fn upstream(mut self, addr: SocketAddr) -> Self {
        self.upstream = Some(addr);
        self
    }

    /// Sets the maximum time to wait for Autumn startup before the first
    /// event. When the time ends, the loop starts. Default: no limit.
    #[must_use]
    pub const fn readiness_timeout(mut self, timeout: Duration) -> Self {
        self.readiness_timeout = Some(timeout);
        self
    }

    /// Sets the time between the end of the upstream call and the
    /// invocation deadline. Default: [`Self::DEFAULT_TIMEOUT_MARGIN`].
    #[must_use]
    pub const fn timeout_margin(mut self, margin: Duration) -> Self {
        self.timeout_margin = margin;
        self
    }

    /// Decides what to do at startup. `env` returns the value of one
    /// variable, or `None`.
    ///
    /// Returns `Ok(None)` if the plugin stays idle. Returns
    /// `Ok(Some(upstream))` if the loop starts.
    pub(crate) fn plan(
        &self,
        server: &ServerConfig,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Upstream>, PluginError> {
        if !self.activation.is_active(env(RUNTIME_API_ENV).is_some()) {
            return Ok(None);
        }
        // In task mode, Autumn does not start the listener.
        if env(TASK_ENV).is_some_and(|task| !task.trim().is_empty()) {
            return Ok(None);
        }
        let names = missing_lambda_env(env);
        if !names.is_empty() {
            return Err(PluginError::MissingEnv { names });
        }
        let upstream = match self.upstream {
            Some(addr) => Upstream::new(addr),
            None => Upstream::from_server_config(server)?,
        };
        Ok(Some(upstream))
    }

    /// Starts the runtime loop if [`plan`](Self::plan) returns an upstream.
    fn start(&self, state: &AppState) -> Result<(), PluginError> {
        let env = |name: &str| std::env::var(name).ok();
        let server = &state.config_arc().server;
        let Some(upstream) = self.plan(server, env)? else {
            tracing::debug!("AWS Lambda is not active; the plugin is idle");
            return Ok(());
        };
        let slow = slow_shutdown_settings(server);
        if !slow.is_empty() {
            tracing::warn!(
                settings = ?slow,
                "Lambda gives little time at shutdown; set these to 0 or 1"
            );
        }
        tracing::info!(%upstream, "AWS Lambda detected; the plugin sends events to Autumn");
        let settings = LoopSettings {
            proxy: LambdaProxy::new(upstream).timeout_margin(self.timeout_margin),
            mode: self.response_mode,
            readiness_timeout: self.readiness_timeout,
            concurrency: lambda_concurrency(env),
            runtime_api: env(RUNTIME_API_ENV).unwrap_or_default(),
        };
        let state = state.clone();
        spawn_loop(settings, move || {
            let probes = state.probes();
            if probes.is_shutting_down() {
                Startup::Stopping
            } else if probes.is_startup_complete() {
                Startup::Complete
            } else {
                Startup::Pending
            }
        })
    }
}

/// Returns the `[server]` settings that are too slow for Lambda shutdown.
///
/// Lambda stops the process soon after `SIGTERM`.
#[must_use]
pub fn slow_shutdown_settings(server: &ServerConfig) -> Vec<&'static str> {
    let mut slow = Vec::new();
    if server.prestop_grace_secs > 0 {
        slow.push("server.prestop_grace_secs");
    }
    if server.shutdown_timeout_secs > 1 {
        slow.push("server.shutdown_timeout_secs");
    }
    slow
}

impl Plugin for AwsLambdaPlugin {
    fn build(self, app: AppBuilder) -> AppBuilder {
        app.on_startup(move |state| {
            let result = self.start(&state).map_err(Into::into);
            async move { result }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::PluginError;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    const LAMBDA_ENV: [(&str, &str); 4] = [
        ("AWS_LAMBDA_RUNTIME_API", "127.0.0.1:9001"),
        ("AWS_LAMBDA_FUNCTION_NAME", "f"),
        ("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "128"),
        ("AWS_LAMBDA_FUNCTION_VERSION", "$LATEST"),
    ];

    #[test]
    fn plugin_registers_once() {
        let app = autumn_web::app()
            .plugin(AwsLambdaPlugin::new())
            .plugin(AwsLambdaPlugin::new());
        assert!(app.has_plugin(std::any::type_name::<AwsLambdaPlugin>()));
    }

    #[test]
    fn auto_is_idle_outside_lambda() {
        let plan = AwsLambdaPlugin::new().plan(&ServerConfig::default(), env(&[]));
        assert!(matches!(plan, Ok(None)));
    }

    #[test]
    fn never_is_idle_on_lambda() {
        let plan = AwsLambdaPlugin::new()
            .activation(Activation::Never)
            .plan(&ServerConfig::default(), env(&LAMBDA_ENV));
        assert!(matches!(plan, Ok(None)));
    }

    #[test]
    fn always_fails_outside_lambda() {
        let plan = AwsLambdaPlugin::new()
            .activation(Activation::Always)
            .plan(&ServerConfig::default(), env(&[]));
        assert!(matches!(plan, Err(PluginError::MissingEnv { names, .. }) if names.len() == 4));
    }

    #[test]
    fn auto_fails_on_partial_lambda_env() {
        let plan = AwsLambdaPlugin::new().plan(&ServerConfig::default(), env(&LAMBDA_ENV[..1]));
        assert!(matches!(plan, Err(PluginError::MissingEnv { names, .. }) if names.len() == 3));
    }

    #[test]
    fn auto_on_lambda_uses_server_config() {
        let plan = AwsLambdaPlugin::new()
            .plan(&ServerConfig::default(), env(&LAMBDA_ENV))
            .expect("valid plan")
            .expect("active");
        assert_eq!(plan.to_string(), "http://127.0.0.1:3000");
    }

    #[test]
    fn upstream_override_wins() {
        let addr: SocketAddr = "0.0.0.0:8080".parse().expect("addr");
        let plan = AwsLambdaPlugin::new()
            .upstream(addr)
            .plan(&ServerConfig::default(), env(&LAMBDA_ENV))
            .expect("valid plan")
            .expect("active");
        assert_eq!(plan.to_string(), "http://127.0.0.1:8080");
    }

    #[test]
    fn task_mode_is_idle_on_lambda() {
        let mut pairs = LAMBDA_ENV.to_vec();
        pairs.push(("AUTUMN_RUN_TASK", "backfill"));
        let plan = AwsLambdaPlugin::new().plan(&ServerConfig::default(), env(&pairs));
        assert!(matches!(plan, Ok(None)));
    }

    #[test]
    fn blank_task_name_is_not_task_mode() {
        let mut pairs = LAMBDA_ENV.to_vec();
        pairs.push(("AUTUMN_RUN_TASK", "  "));
        let plan = AwsLambdaPlugin::new().plan(&ServerConfig::default(), env(&pairs));
        assert!(matches!(plan, Ok(Some(_))));
    }

    #[test]
    fn always_starts_on_lambda() {
        let plan = AwsLambdaPlugin::new()
            .activation(Activation::Always)
            .plan(&ServerConfig::default(), env(&LAMBDA_ENV));
        assert!(matches!(plan, Ok(Some(_))));
    }

    #[test]
    fn upstream_override_skips_server_checks() {
        let server = ServerConfig {
            port: 0,
            unix_socket: Some("/tmp/app.sock".to_owned()),
            ..ServerConfig::default()
        };
        let addr: SocketAddr = "127.0.0.1:9000".parse().expect("addr");
        let plan = AwsLambdaPlugin::new()
            .upstream(addr)
            .plan(&server, env(&LAMBDA_ENV));
        assert!(matches!(plan, Ok(Some(up)) if up.addr() == addr));
    }

    #[test]
    fn server_errors_stop_the_plan() {
        let server = ServerConfig {
            port: 0,
            ..ServerConfig::default()
        };
        let plan = AwsLambdaPlugin::new().plan(&server, env(&LAMBDA_ENV));
        assert!(matches!(plan, Err(PluginError::ZeroPort)));
    }

    #[test]
    fn default_equals_new() {
        let a = AwsLambdaPlugin::default();
        assert_eq!(a.readiness_timeout, None);
        assert_eq!(a.timeout_margin, AwsLambdaPlugin::DEFAULT_TIMEOUT_MARGIN);
        assert_eq!(a.activation, Activation::Auto);
        assert_eq!(a.response_mode, ResponseMode::Buffered);
        assert!(a.upstream.is_none());
    }
}
