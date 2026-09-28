//! The Autumn plugin.

use std::net::SocketAddr;
use std::time::Duration;

use autumn_web::AppState;
use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;

use crate::activation::missing_lambda_env;
use crate::runtime::{run, wait_until};
use crate::{Activation, LambdaProxy, PluginError, RUNTIME_API_ENV, ResponseMode, Upstream};

/// First delay of the readiness wait.
const READY_INITIAL: Duration = Duration::from_millis(5);
/// Longest delay of the readiness wait.
const READY_MAX: Duration = Duration::from_millis(100);

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

impl AwsLambdaPlugin {
    /// Decides what to do at startup. `env` reads one variable.
    ///
    /// Returns `Ok(None)` when the plugin must stay idle, and the upstream
    /// when the loop must start.
    pub(crate) fn plan(
        &self,
        state: &AppState,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Upstream>, PluginError> {
        if !self.activation.is_active(env(RUNTIME_API_ENV).is_some()) {
            return Ok(None);
        }
        let names = missing_lambda_env(env);
        if !names.is_empty() {
            return Err(PluginError::MissingEnv { names });
        }
        let upstream = match self.upstream {
            Some(addr) => Upstream::new(addr),
            None => Upstream::from_server_config(&state.config_arc().server)?,
        };
        Ok(Some(upstream))
    }

    /// Starts the runtime loop in a task when [`plan`](Self::plan) says so.
    fn start(&self, state: &AppState) -> Result<(), PluginError> {
        let Some(upstream) = self.plan(state, |name| std::env::var(name).ok())? else {
            tracing::debug!("AWS Lambda not active; plugin is idle");
            return Ok(());
        };
        let proxy = LambdaProxy::new(upstream).timeout_margin(self.timeout_margin);
        let mode = self.response_mode;
        let readiness_timeout = self.readiness_timeout;
        let probes = state.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(PluginError::Runtime)?;
        // The `lambda_http` loop future is not `Send`, so it cannot go to
        // `tokio::spawn`. It runs on its own thread and runtime.
        std::thread::Builder::new()
            .name("aws-lambda-runtime".to_owned())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async move {
                        let ready = wait_until(
                            || probes.probes().is_startup_complete(),
                            readiness_timeout,
                            READY_INITIAL,
                            READY_MAX,
                        )
                        .await;
                        if !ready {
                            tracing::warn!(
                                ?readiness_timeout,
                                "Autumn startup not complete; starting Lambda loop anyway"
                            );
                        }
                        tracing::info!(%upstream, ?mode, "starting AWS Lambda runtime loop");
                        run(proxy, mode).await
                    })
                }));
                // The loop runs forever. If it stops, Lambda must get a new
                // execution environment.
                tracing::error!(?outcome, "AWS Lambda runtime loop stopped; exiting");
                std::process::exit(1);
            })
            .map_err(PluginError::Runtime)?;
        Ok(())
    }
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

    use autumn_web::AppState;

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
        let plan = AwsLambdaPlugin::new().plan(&AppState::for_test(), env(&[]));
        assert_eq!(plan, Ok(None));
    }

    #[test]
    fn never_is_idle_on_lambda() {
        let plan = AwsLambdaPlugin::new()
            .activation(Activation::Never)
            .plan(&AppState::for_test(), env(&LAMBDA_ENV));
        assert_eq!(plan, Ok(None));
    }

    #[test]
    fn always_fails_outside_lambda() {
        let plan = AwsLambdaPlugin::new()
            .activation(Activation::Always)
            .plan(&AppState::for_test(), env(&[]));
        assert!(matches!(plan, Err(PluginError::MissingEnv { names }) if names.len() == 4));
    }

    #[test]
    fn auto_fails_on_partial_lambda_env() {
        let plan = AwsLambdaPlugin::new().plan(&AppState::for_test(), env(&LAMBDA_ENV[..1]));
        assert!(matches!(plan, Err(PluginError::MissingEnv { names }) if names.len() == 3));
    }

    #[test]
    fn auto_on_lambda_uses_server_config() {
        let plan = AwsLambdaPlugin::new()
            .plan(&AppState::for_test(), env(&LAMBDA_ENV))
            .expect("valid plan")
            .expect("active");
        assert_eq!(plan.to_string(), "http://127.0.0.1:3000");
    }

    #[test]
    fn upstream_override_wins() {
        let addr: SocketAddr = "0.0.0.0:8080".parse().expect("addr");
        let plan = AwsLambdaPlugin::new()
            .upstream(addr)
            .plan(&AppState::for_test(), env(&LAMBDA_ENV))
            .expect("valid plan")
            .expect("active");
        assert_eq!(plan.to_string(), "http://127.0.0.1:8080");
    }

    #[test]
    fn default_equals_new() {
        let a = AwsLambdaPlugin::default();
        assert_eq!(
            a.readiness_timeout,
            AwsLambdaPlugin::DEFAULT_READINESS_TIMEOUT
        );
        assert_eq!(a.timeout_margin, AwsLambdaPlugin::DEFAULT_TIMEOUT_MARGIN);
        assert_eq!(a.activation, Activation::Auto);
        assert_eq!(a.response_mode, ResponseMode::Buffered);
        assert!(a.upstream.is_none());
    }
}
