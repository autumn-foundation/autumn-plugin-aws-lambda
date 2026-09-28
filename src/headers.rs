//! Header rules for the proxy. Verus twin: `verification/core.rs::filter_headers`.

use http::HeaderMap;
use http::header::HeaderName;

/// The header that carries the request ID to the app.
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Removes hop-by-hop headers (RFC 9110, section 7.6.1).
///
/// This removes the fixed list and each name in the `Connection` header.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let _ = headers;
    todo!()
}

/// Prepares request headers for the upstream call.
///
/// It removes hop-by-hop headers and `content-length`. The client sets
/// `content-length` again from the body. It sets [`REQUEST_ID_HEADER`] to
/// `request_id` when the header is not present.
pub fn prepare_request_headers(headers: &mut HeaderMap, request_id: Option<&str>) {
    let _ = (headers, request_id);
    todo!()
}
