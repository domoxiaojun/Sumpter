//! Inbound HTTP boundary helpers.
//!
//! Platform code must not be mixed into this module.  It owns only lossless
//! request-envelope conversion and path extraction; authentication and body
//! consumption stay in the parent engine pipeline.

use axum::http::{HeaderMap, Uri};

pub use crate::boundary::InboundRequest;

/// Return the request target including its query, defaulting to `/` for an
/// origin-form URI without a path-and-query component.
pub fn path_and_query(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string())
}

/// Convert an HTTP header map to the legacy ordered pair representation used
/// by the protocol adapters.  Values are lossy-decoded exactly as the old
/// server facade did; no body bytes are touched here.
pub fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).to_string(),
            )
        })
        .collect()
}
