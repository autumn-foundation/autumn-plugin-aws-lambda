//! The proxy service: Lambda request in, upstream response out.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::uri::PathAndQuery;
use http::{HeaderValue, Request, Response, StatusCode, Uri, Version, header};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use lambda_http::RequestExt;

use crate::Upstream;
use crate::headers::{prepare_request_headers, strip_hop_by_hop};
use crate::timing::budget_until;

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
        match self {
            Self::Connect | Self::Body => StatusCode::BAD_GATEWAY,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    /// Makes the plain-text response for this failure.
    fn response(self) -> Response<ProxyBody> {
        let status = self.status();
        let text = status.canonical_reason().unwrap_or("Gateway Error");
        let mut resp = Response::new(full(Bytes::from_static(text.as_bytes())));
        *resp.status_mut() = status;
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        resp
    }
}

/// A `tower` service that sends Lambda HTTP requests to the upstream.
///
/// It never returns an error. Upstream failures become `502` or `504`
/// responses.
#[derive(Debug, Clone)]
pub struct LambdaProxy {
    client: Client<HttpConnector, Full<Bytes>>,
    upstream: Upstream,
    timeout_margin: Duration,
    buffer_body: bool,
}

impl LambdaProxy {
    /// Makes a proxy to `upstream`.
    #[must_use]
    pub fn new(upstream: Upstream) -> Self {
        Self {
            client: Client::builder(TokioExecutor::new()).build_http(),
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

    /// Sends one request. The deadline, if known, applies to the whole call.
    async fn forward(self, req: lambda_http::Request) -> Result<Response<ProxyBody>, ProxyFailure> {
        let context = req.lambda_context_ref();
        let deadline = context.map(|c| c.deadline);
        let request_id = context.map(|c| c.request_id.clone());

        let (mut parts, body) = req.into_parts();
        parts.uri = self.upstream_uri(parts.uri.path_and_query())?;
        parts.version = Version::HTTP_11;
        parts.extensions = http::Extensions::new();
        prepare_request_headers(&mut parts.headers, request_id.as_deref());
        let upstream_req = Request::from_parts(parts, Full::new(body_bytes(body)));

        let call = self.send(upstream_req);
        match deadline {
            Some(deadline) => {
                tokio::time::timeout(budget_until(deadline, self.timeout_margin), call)
                    .await
                    .map_err(|_| ProxyFailure::Timeout)?
            }
            None => call.await,
        }
    }

    async fn send(&self, req: Request<Full<Bytes>>) -> Result<Response<ProxyBody>, ProxyFailure> {
        let resp = self.client.request(req).await.map_err(|error| {
            tracing::warn!(upstream = %self.upstream, %error, "upstream request failed");
            ProxyFailure::Connect
        })?;
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        let body = if self.buffer_body {
            let bytes = body
                .collect()
                .await
                .map_err(|error| {
                    tracing::warn!(upstream = %self.upstream, %error, "upstream body failed");
                    ProxyFailure::Body
                })?
                .to_bytes();
            full(bytes)
        } else {
            body.boxed()
        };
        Ok(Response::from_parts(parts, body))
    }

    fn upstream_uri(&self, path: Option<&PathAndQuery>) -> Result<Uri, ProxyFailure> {
        Uri::builder()
            .scheme("http")
            .authority(self.upstream.addr().to_string())
            .path_and_query(path.map_or("/", PathAndQuery::as_str))
            .build()
            .map_err(|_| ProxyFailure::Connect)
    }
}

impl tower_service::Service<lambda_http::Request> for LambdaProxy {
    type Response = Response<ProxyBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: lambda_http::Request) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            Ok(this
                .forward(req)
                .await
                .unwrap_or_else(ProxyFailure::response))
        })
    }
}

fn body_bytes(body: lambda_http::Body) -> Bytes {
    match body {
        lambda_http::Body::Empty => Bytes::new(),
        lambda_http::Body::Text(text) => Bytes::from(text),
        lambda_http::Body::Binary(bytes) => Bytes::from(bytes),
        other => Bytes::copy_from_slice(other.as_ref()),
    }
}

fn full(bytes: Bytes) -> ProxyBody {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}
