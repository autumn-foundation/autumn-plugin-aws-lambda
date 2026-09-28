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
        match self {
            Self::Auto => runtime_api_present,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

/// Variables that `lambda_runtime` reads at start. It panics if one is missing.
const REQUIRED_ENV: [&str; 4] = [
    RUNTIME_API_ENV,
    "AWS_LAMBDA_FUNCTION_NAME",
    "AWS_LAMBDA_FUNCTION_MEMORY_SIZE",
    "AWS_LAMBDA_FUNCTION_VERSION",
];

/// Returns the required Lambda variables that are missing or not valid.
///
/// `env` reads one variable. An empty result means the runtime can start.
pub fn missing_lambda_env(env: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
    REQUIRED_ENV
        .into_iter()
        .filter(|name| {
            env(name).is_none_or(|value| {
                *name == "AWS_LAMBDA_FUNCTION_MEMORY_SIZE" && value.parse::<i32>().is_err()
            })
        })
        .collect()
}
