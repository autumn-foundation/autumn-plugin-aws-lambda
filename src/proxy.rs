//! The proxy service. It sends a Lambda request to the upstream and
//! returns the response.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderValue, Request, Response, StatusCode, Version, header};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use lambda_http::RequestExt;
use lambda_http::request::RequestContext;

use crate::ResponseMode;
use crate::headers::{
    fold_repeated_headers, prepare_request_headers, set_forwarded_for, strip_hop_by_hop,
    text_body_is_lossless, upstream_path_and_query,
};
use crate::timing::budget_until;
use crate::upstream::Upstream;

/// The response body type of [`LambdaProxy`].
pub type ProxyBody = BoxBody<Bytes, hyper::Error>;

/// The largest body that buffered mode reads. Lambda does not accept a
/// larger buffered response.
const MAX_BUFFERED_BODY: usize = 6 * 1024 * 1024;

/// Why the upstream call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProxyFailure {
    /// The client cannot connect or cannot send the request.
    Connect,
    /// The call does not end before the invocation deadline.
    Timeout,
    /// The proxy cannot read the response body, or the body is too large.
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
/// Upstream failures before the response head become `502` or `504`
/// responses. The service does not return an error.
#[derive(Debug, Clone)]
pub struct LambdaProxy {
    client: Client<HttpConnector, Full<Bytes>>,
    upstream: Upstream,
    timeout_margin: Duration,
    mode: ResponseMode,
}

impl LambdaProxy {
    /// Makes a proxy to `upstream`. The timeout margin is zero. The mode is
    /// [`ResponseMode::Buffered`].
    #[must_use]
    pub fn new(upstream: Upstream) -> Self {
        Self {
            client: Client::builder(TokioExecutor::new()).build_http(),
            upstream,
            timeout_margin: Duration::ZERO,
            mode: ResponseMode::Buffered,
        }
    }

    /// Sets the time between the end of the upstream call and the
    /// invocation deadline.
    #[must_use]
    pub const fn timeout_margin(mut self, margin: Duration) -> Self {
        self.timeout_margin = margin;
        self
    }

    /// Sets the response mode.
    ///
    /// - `Buffered`: the proxy reads the full body (up to 6 MiB) before it
    ///   returns. The deadline applies to the body too.
    /// - `Streaming`: the proxy returns after the head. The deadline
    ///   applies to the head only. A body error then ends the stream.
    #[must_use]
    pub const fn response_mode(mut self, mode: ResponseMode) -> Self {
        self.mode = mode;
        self
    }

    /// Sends one request. The deadline, if known, applies to the call.
    async fn forward(self, req: lambda_http::Request) -> Result<Response<ProxyBody>, ProxyFailure> {
        let context = req.lambda_context_ref();
        let deadline = context.map(|c| c.deadline);
        let request_id = context.map(|c| c.request_id.clone());
        let source_ip = source_ip(&req);
        let raw_path = Some(req.raw_http_path()).filter(|p| !p.is_empty());
        let path = upstream_path_and_query(raw_path, req.uri());

        let (mut parts, body) = req.into_parts();
        parts.uri = format!("http://{}{path}", self.upstream.addr())
            .parse()
            .map_err(|_| ProxyFailure::Connect)?;
        parts.version = Version::HTTP_11;
        parts.extensions = http::Extensions::new();
        prepare_request_headers(&mut parts.headers, request_id.as_deref());
        set_forwarded_for(&mut parts.headers, source_ip.as_deref());
        let upstream_req = Request::from_parts(parts, Full::new(body_bytes(body)));

        let Some(deadline) = deadline else {
            return self.send(upstream_req).await;
        };
        let budget = budget_until(deadline, self.timeout_margin);
        if budget.is_zero() {
            // No time is left. Do not call the upstream.
            return Err(ProxyFailure::Timeout);
        }
        tokio::time::timeout(budget, self.send(upstream_req))
            .await
            .map_err(|_| ProxyFailure::Timeout)?
    }

    async fn send(&self, req: Request<Full<Bytes>>) -> Result<Response<ProxyBody>, ProxyFailure> {
        let resp = self.client.request(req).await.map_err(|error| {
            tracing::warn!(upstream = %self.upstream, %error, "upstream request failed");
            ProxyFailure::Connect
        })?;
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        fold_repeated_headers(&mut parts.headers);
        let body = match self.mode {
            ResponseMode::Streaming => body.boxed(),
            ResponseMode::Buffered => {
                let bytes = Limited::new(body, MAX_BUFFERED_BODY)
                    .collect()
                    .await
                    .map_err(|error| {
                        tracing::warn!(upstream = %self.upstream, %error, "upstream body failed");
                        ProxyFailure::Body
                    })?
                    .to_bytes();
                // The length can change when lambda_http encodes the body.
                parts.headers.remove(header::CONTENT_LENGTH);
                let content_type = parts
                    .headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok());
                if !parts.headers.contains_key(header::CONTENT_ENCODING)
                    && !text_body_is_lossless(content_type, &bytes)
                {
                    // lambda_http sends a body with any content-encoding as
                    // base64 binary, with no charset change.
                    parts.headers.insert(
                        header::CONTENT_ENCODING,
                        HeaderValue::from_static("identity"),
                    );
                }
                full(bytes)
            }
        };
        Ok(Response::from_parts(parts, body))
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

/// Returns the client IP from the API Gateway request context.
fn source_ip(req: &lambda_http::Request) -> Option<String> {
    match req.request_context_ref()? {
        RequestContext::ApiGatewayV1(ctx) => ctx.identity.source_ip.clone(),
        RequestContext::ApiGatewayV2(ctx) => ctx.http.source_ip.clone(),
        RequestContext::WebSocket(ctx) => ctx.identity.source_ip.clone(),
        _ => None,
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
