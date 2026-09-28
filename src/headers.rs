//! Header rules for the proxy. Verus twin: `verification/core.rs::filter_headers`.

use http::header::{
    CONNECTION, CONTENT_LENGTH, HeaderName, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER,
    TRANSFER_ENCODING, UPGRADE,
};
use http::{HeaderMap, HeaderValue};

/// The header that carries the request ID to the app.
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Hop-by-hop headers (RFC 9110, section 7.6.1, and common legacy names).
const HOP_BY_HOP: [HeaderName; 9] = [
    CONNECTION,
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-connection"),
    TRANSFER_ENCODING,
    TE,
    TRAILER,
    UPGRADE,
    PROXY_AUTHENTICATE,
    PROXY_AUTHORIZATION,
];

/// Removes hop-by-hop headers (RFC 9110, section 7.6.1).
///
/// This removes the fixed list and each name in the `Connection` header.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|token| HeaderName::from_bytes(token.trim().as_bytes()).ok())
        .collect();
    for name in HOP_BY_HOP.iter().chain(&listed) {
        headers.remove(name);
    }
}

/// Prepares request headers for the upstream call.
///
/// It removes hop-by-hop headers and `content-length`. The client sets
/// `content-length` again from the body. It sets [`REQUEST_ID_HEADER`] to
/// `request_id` when the header is not present.
pub fn prepare_request_headers(headers: &mut HeaderMap, request_id: Option<&str>) {
    strip_hop_by_hop(headers);
    headers.remove(CONTENT_LENGTH);
    if headers.contains_key(REQUEST_ID_HEADER) {
        return;
    }
    if let Some(value) = request_id.and_then(|id| HeaderValue::from_str(id).ok()) {
        headers.insert(REQUEST_ID_HEADER, value);
    }
}
