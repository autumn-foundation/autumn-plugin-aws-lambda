//! Errors.

/// Errors that stop the plugin at startup.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// `server.unix_socket` is set. The proxy needs TCP.
    #[error("the Lambda plugin does not support server.unix_socket; remove it")]
    UnixSocketUnsupported,
    /// `server.tls` is set. The proxy sends plain HTTP.
    #[error("the Lambda plugin does not support server.tls; Lambda terminates TLS")]
    TlsUnsupported,
    /// `server.port` is `0`. The plugin cannot know the port.
    #[error("server.port is 0; set a fixed port")]
    ZeroPort,
    /// Required Lambda environment variables are missing or not valid.
    #[error(
        "the Lambda environment is not complete; missing or not valid: {}",
        names.join(", ")
    )]
    #[non_exhaustive]
    MissingEnv {
        /// The names of the missing or incorrect variables.
        names: Vec<&'static str>,
    },
    /// The plugin cannot start the runtime thread.
    #[error("cannot start the Lambda runtime thread: {0}")]
    Runtime(#[source] std::io::Error),
    /// `server.host` is not an IP address or `localhost`.
    #[error(
        "server.host {host:?} is not an IP address or localhost; set AwsLambdaPlugin::upstream"
    )]
    #[non_exhaustive]
    InvalidHost {
        /// The value of `server.host`.
        host: String,
    },
}
