//! When the Lambda runtime loop starts.

/// The environment variable that Lambda sets for the Runtime API.
pub const RUNTIME_API_ENV: &str = "AWS_LAMBDA_RUNTIME_API";

/// Controls when the plugin starts the Lambda runtime loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Activation {
    /// Start only when [`RUNTIME_API_ENV`] is set. This is the default.
    #[default]
    Auto,
    /// Always start. Startup fails outside Lambda.
    Always,
    /// Never start. Use this to turn the plugin off.
    Never,
}

impl Activation {
    /// Returns `true` when the loop must start.
    #[must_use]
    pub const fn is_active(self, runtime_api_present: bool) -> bool {
        let _ = runtime_api_present;
        todo!()
    }

    /// Returns `true` when the loop must start in this process.
    #[must_use]
    pub fn is_active_in_env(self) -> bool {
        self.is_active(std::env::var_os(RUNTIME_API_ENV).is_some())
    }
}
