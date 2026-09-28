//! Errors.

/// Errors that stop the plugin at startup.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// `server.unix_socket` is set. The proxy needs TCP.
    #[error(
        "server.unix_socket is not supported on Lambda; remove it or set AwsLambdaPlugin::upstream"
    )]
    UnixSocketUnsupported,
    /// `server.tls` is set. The proxy sends plain HTTP.
    #[error("server.tls is not supported on Lambda; Lambda terminates TLS")]
    TlsUnsupported,
    /// `server.port` is `0`. The plugin cannot know the port.
    #[error("server.port is 0; set a fixed port or AwsLambdaPlugin::upstream")]
    ZeroPort,
    /// Lambda variables are missing or not valid.
    #[error("Lambda environment is not complete; missing or invalid: {}", names.join(", "))]
    MissingEnv {
        /// The variable names.
        names: Vec<&'static str>,
    },
    /// The runtime thread could not start.
    #[error("cannot start the Lambda runtime thread: {0}")]
    Runtime(#[source] std::io::Error),
    /// `server.host` is not an IP address or `localhost`.
    #[error("server.host {host:?} is not an IP address; set AwsLambdaPlugin::upstream")]
    InvalidHost {
        /// The configured host.
        host: String,
    },
}

impl PartialEq for PluginError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Runtime(a), Self::Runtime(b)) => a.kind() == b.kind(),
            (Self::MissingEnv { names: a }, Self::MissingEnv { names: b }) => a == b,
            (Self::InvalidHost { host: a }, Self::InvalidHost { host: b }) => a == b,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

impl Eq for PluginError {}
