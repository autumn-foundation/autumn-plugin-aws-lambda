//! When the Lambda runtime loop starts. Verus twin: `Activation` in
//! `verification/core.rs`.

/// The variable that Lambda sets to the Runtime API address.
pub const RUNTIME_API_ENV: &str = "AWS_LAMBDA_RUNTIME_API";

/// The variable that sets the number of events that one environment
/// handles at the same time (Lambda Managed Instances).
const MAX_CONCURRENCY_ENV: &str = "AWS_LAMBDA_MAX_CONCURRENCY";

/// Controls when the plugin starts the Lambda runtime loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Activation {
    /// Start only when Lambda sets `AWS_LAMBDA_RUNTIME_API`. This is the default.
    #[default]
    Auto,
    /// Always start. Startup fails if a Lambda variable is missing.
    Always,
    /// Do not start. Use this to keep the plugin idle.
    Never,
}

impl Activation {
    /// Returns `true` if the loop must start.
    pub(crate) const fn is_active(self, runtime_api_present: bool) -> bool {
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
/// `env` returns the value of one variable, or `None`. The memory size
/// must be an `i32`. An empty result means that the runtime can start.
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

/// Returns the number of events to handle at the same time.
///
/// The rule is the same as in `lambda_runtime`: a positive `u32`, else 1.
pub fn lambda_concurrency(env: impl Fn(&str) -> Option<String>) -> u32 {
    env(MAX_CONCURRENCY_ENV)
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::Activation;

    #[test]
    fn auto_starts_only_on_lambda() {
        assert!(Activation::Auto.is_active(true));
        assert!(!Activation::Auto.is_active(false));
    }

    #[test]
    fn always_and_never_ignore_the_environment() {
        for present in [true, false] {
            assert!(Activation::Always.is_active(present));
            assert!(!Activation::Never.is_active(present));
        }
    }

    #[test]
    fn default_activation_is_auto() {
        assert_eq!(Activation::default(), Activation::Auto);
    }
}
