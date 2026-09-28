//! The proxy service: Lambda request in, upstream response out.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::StatusCode;
use http_body_util::combinators::BoxBody;

use crate::Upstream;

/// The response body type of [`LambdaProxy`].
pub type ProxyBody = BoxBody<Bytes, hyper::Error>;

/// Why the upstream call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyFailure {
    /// The client could not connect or send the request.
    Connect,
    /// The call did not finish before the invocation deadline.
    Timeout,
    /// The response body could not be read.
    Body,
}

impl ProxyFailure {
    /// Returns the HTTP status for this failure: `502` or `504`.
    #[must_use]
    pub const fn status(self) -> StatusCode {
        todo!()
    }
}

/// A `tower` service that sends Lambda HTTP requests to the upstream.
///
/// It never returns an error. Upstream failures become `502` or `504`
/// responses.
#[derive(Debug, Clone)]
pub struct LambdaProxy {
    upstream: Upstream,
    timeout_margin: Duration,
    buffer_body: bool,
}

impl LambdaProxy {
    /// Makes a proxy to `upstream`.
    #[must_use]
    pub fn new(upstream: Upstream) -> Self {
        Self {
            upstream,
            timeout_margin: Duration::ZERO,
            buffer_body: true,
        }
    }

    /// Sets the time to keep free before the invocation deadline.
    #[must_use]
    pub const fn timeout_margin(mut self, margin: Duration) -> Self {
        self.timeout_margin = margin;
        self
    }

    /// When `true`, reads the full body before it returns. The deadline
    /// then also applies to the body. When `false`, streams the body.
    #[must_use]
    pub const fn buffer_body(mut self, buffer: bool) -> Self {
        self.buffer_body = buffer;
        self
    }
}

impl tower_service::Service<lambda_http::Request> for LambdaProxy {
    type Response = http::Response<ProxyBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: lambda_http::Request) -> Self::Future {
        let _ = req;
        todo!()
    }
}
