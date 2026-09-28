//! Header and path rules for the proxy.
//! Verus twin of the filter: `verification/core.rs::filter_headers`.

use std::net::IpAddr;

use http::header::{
    CONNECTION, CONTENT_LENGTH, FORWARDED, HeaderName, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION,
    SET_COOKIE, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use http::{HeaderMap, HeaderValue, Uri};

/// The header that carries the Lambda request ID to the app.
///
/// The proxy always sets it. A value from the client is removed.
pub const LAMBDA_REQUEST_ID_HEADER: HeaderName =
    HeaderName::from_static("lambda-runtime-aws-request-id");

const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
const X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");

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

/// Removes hop-by-hop headers from a response (RFC 9110, section 7.6.1).
///
/// This removes the fixed list and each name in the `Connection` header.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .flat_map(|value| value.as_bytes().split(|b| *b == b','))
        .filter_map(|token| HeaderName::from_bytes(token.trim_ascii()).ok())
        .collect();
    for name in HOP_BY_HOP.iter().chain(&listed) {
        headers.remove(name);
    }
}

/// Prepares request headers for the upstream call.
///
/// - It removes the fixed hop-by-hop list. The event is not a hop, so
///   names in `Connection` stay. A client cannot delete `Host` this way.
/// - It removes `content-length`. The client sets it again from the body.
/// - It removes `x-forwarded-host`, `x-real-ip`, and `forwarded`. A client
///   can set these, and AWS does not replace them.
/// - It sets [`LAMBDA_REQUEST_ID_HEADER`] to `request_id`, or removes it.
pub fn prepare_request_headers(headers: &mut HeaderMap, request_id: Option<&str>) {
    for name in HOP_BY_HOP
        .iter()
        .chain(&[CONTENT_LENGTH, X_FORWARDED_HOST, X_REAL_IP, FORWARDED])
    {
        headers.remove(name);
    }
    headers.remove(LAMBDA_REQUEST_ID_HEADER);
    if let Some(value) = request_id.and_then(|id| HeaderValue::from_str(id).ok()) {
        headers.insert(LAMBDA_REQUEST_ID_HEADER, value);
    }
}

/// Returns the client IP that AWS saw.
///
/// `source_ip` comes from the API Gateway request context. When it is not
/// there (ALB, VPC Lattice), the last `x-forwarded-for` entry is used,
/// because the load balancer appends the peer address.
#[must_use]
pub fn forwarded_client_ip(source_ip: Option<&str>, forwarded_for: Option<&str>) -> Option<IpAddr> {
    match source_ip {
        Some(ip) => ip.trim().parse().ok(),
        None => forwarded_for?.rsplit(',').next()?.trim().parse().ok(),
    }
}

/// Sets `x-forwarded-for` to one trusted value: the client IP that AWS saw.
///
/// It removes the header when no valid IP is known.
pub fn set_forwarded_for(headers: &mut HeaderMap, source_ip: Option<&str>) {
    let forwarded_for = headers
        .get(X_FORWARDED_FOR)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    headers.remove(X_FORWARDED_FOR);
    if let Some(ip) = forwarded_client_ip(source_ip, forwarded_for.as_deref())
        && let Ok(value) = HeaderValue::from_str(&ip.to_string())
    {
        headers.insert(X_FORWARDED_FOR, value);
    }
}

/// Joins the values of each repeated header into one value, except
/// `set-cookie` (RFC 9110, section 5.3).
///
/// Some Lambda event sources keep only the first value of a header.
pub fn fold_repeated_headers(headers: &mut HeaderMap) {
    let repeated: Vec<HeaderName> = headers
        .keys()
        .filter(|name| **name != SET_COOKIE && headers.get_all(*name).iter().nth(1).is_some())
        .cloned()
        .collect();
    for name in repeated {
        let joined = headers
            .get_all(&name)
            .iter()
            .map(HeaderValue::as_bytes)
            .collect::<Vec<_>>()
            .join(&b", "[..]);
        if let Ok(value) = HeaderValue::from_bytes(&joined) {
            headers.insert(name, value);
        }
    }
}

/// Returns `true` when `lambda_http` can send `body` as text with no change.
///
/// `lambda_http` decodes text bodies with the charset of `content_type`
/// and sends UTF-8. That changes the bytes unless they are ASCII, or UTF-8
/// with a UTF-8 (or no) charset.
#[must_use]
pub fn text_body_is_lossless(content_type: Option<&str>, body: &[u8]) -> bool {
    if body.is_ascii() {
        return true;
    }
    let charset = content_type.and_then(|ct| {
        ct.split(';').skip(1).find_map(|param| {
            let (key, value) = param.split_once('=')?;
            key.trim()
                .eq_ignore_ascii_case("charset")
                .then(|| value.trim().trim_matches('"').to_ascii_lowercase())
        })
    });
    matches!(charset.as_deref(), None | Some("utf-8" | "utf8")) && std::str::from_utf8(body).is_ok()
}

/// Returns the path and query for the upstream request.
///
/// It uses `raw_path` from the event, so the upstream sees the path that
/// AWS routed: no dot-segment removal and no API Gateway stage prefix.
/// It encodes bytes that a URI path cannot hold. Without `raw_path`, it
/// uses the path of `uri`. The query always comes from `uri`.
#[must_use]
pub fn upstream_path_and_query(raw_path: Option<&str>, uri: &Uri) -> String {
    let mut out = String::new();
    match raw_path {
        Some(raw) => {
            if !raw.starts_with('/') {
                out.push('/');
            }
            encode_path(raw, &mut out);
        }
        None => out.push_str(uri.path()),
    }
    if let Some(query) = uri.query().filter(|q| !q.is_empty()) {
        out.push('?');
        out.push_str(query);
    }
    out
}

/// Appends `raw` to `out`. Encodes bytes that are not valid in a URI path.
/// Keeps `%XX` escapes that are already there.
/// Verus twin: `verification/core.rs::encode_path`.
fn encode_path(raw: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%'
            && bytes.len() - i > 2
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            out.push_str(&raw[i..i + 3]);
            i += 3;
        } else if is_path_byte(b) {
            out.push(char::from(b));
            i += 1;
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b / 16)]));
            out.push(char::from(HEX[usize::from(b % 16)]));
            i += 1;
        }
    }
}

/// Returns `true` for a byte that a URI path can hold as it is (RFC 3986
/// `pchar` and `/`, but not `%`).
const fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
                | b'/'
        )
}
