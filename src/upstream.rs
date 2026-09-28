//! The address of the local Autumn server.

use std::fmt;
use std::net::SocketAddr;

use autumn_web::config::ServerConfig;

use crate::PluginError;

/// The TCP address of the local Autumn server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Upstream(SocketAddr);

impl Upstream {
    /// Makes an upstream from an address.
    ///
    /// An unspecified IP (`0.0.0.0` or `::`) changes to loopback.
    #[must_use]
    pub const fn new(addr: SocketAddr) -> Self {
        let _ = addr;
        todo!()
    }

    /// Makes an upstream from Autumn `[server]` config.
    ///
    /// # Errors
    ///
    /// Returns an error when `unix_socket` or `tls` is set, or when `host`
    /// is not an IP address or `localhost`.
    pub fn from_server_config(server: &ServerConfig) -> Result<Self, PluginError> {
        let _ = server;
        todo!()
    }

    /// Returns the socket address.
    #[must_use]
    pub const fn addr(self) -> SocketAddr {
        self.0
    }
}

impl fmt::Display for Upstream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http://{}", self.0)
    }
}
