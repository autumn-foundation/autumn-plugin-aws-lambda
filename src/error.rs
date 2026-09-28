//! Errors.

/// Errors that stop the plugin at startup.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginError {
    /// `server.unix_socket` is set. The proxy needs TCP.
    #[error("server.unix_socket is not supported on Lambda; remove it or set AwsLambdaPlugin::upstream")]
    UnixSocketUnsupported,
    /// `server.tls` is set. The proxy sends plain HTTP.
    #[error("server.tls is not supported on Lambda; Lambda terminates TLS")]
    TlsUnsupported,
    /// `server.host` is not an IP address or `localhost`.
    #[error("server.host {host:?} is not an IP address; set AwsLambdaPlugin::upstream")]
    InvalidHost {
        /// The configured host.
        host: String,
    },
}
