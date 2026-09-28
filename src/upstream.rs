//! The address of the local Autumn server.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use autumn_web::config::ServerConfig;

use crate::PluginError;

/// The TCP address of the local Autumn server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Upstream(SocketAddr);

impl Upstream {
    /// Makes an upstream from an address.
    ///
    /// The function changes an unspecified IP (`0.0.0.0` or `::`) to loopback.
    #[must_use]
    pub const fn new(addr: SocketAddr) -> Self {
        let ip = match addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        Self(SocketAddr::new(ip, addr.port()))
    }

    /// Makes an upstream from Autumn `[server]` config.
    ///
    /// # Errors
    ///
    /// Returns an error in these conditions:
    ///
    /// - `unix_socket` is set.
    /// - `tls` is set.
    /// - `port` is `0`.
    /// - `host` is not an IP address or `localhost`.
    pub fn from_server_config(server: &ServerConfig) -> Result<Self, PluginError> {
        if server.unix_socket.is_some() {
            return Err(PluginError::UnixSocketUnsupported);
        }
        if server.tls.is_some() {
            return Err(PluginError::TlsUnsupported);
        }
        if server.port == 0 {
            return Err(PluginError::ZeroPort);
        }
        let host = server.host.trim();
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        let ip = if host.eq_ignore_ascii_case("localhost") {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            host.parse().map_err(|_| PluginError::InvalidHost {
                host: server.host.clone(),
            })?
        };
        Ok(Self::new(SocketAddr::new(ip, server.port)))
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
