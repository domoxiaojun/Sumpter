//! HTTP connection-scoped fields must never cross a proxy hop.
use std::collections::HashSet;

pub fn connection_fields(headers: &[(String, String)]) -> HashSet<String> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .filter_map(|token| {
            let token = token.trim();
            // Reject invalid tokens individually, preserving valid siblings.
            http::HeaderName::from_bytes(token.as_bytes())
                .ok()
                .map(|name| name.as_str().to_owned())
        })
        .collect()
}

pub fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_all_connection_lines_and_valid_tokens_only() {
        let fields = connection_fields(&[
            (
                "Connection".into(),
                " X-Private, ,bad token, x-another".into(),
            ),
            ("cONNection".into(), "x-private, Upgrade, :invalid".into()),
        ]);
        assert_eq!(
            fields,
            HashSet::from(["x-private".into(), "x-another".into(), "upgrade".into()])
        );
        assert!(is_hop_by_hop("Transfer-Encoding"));
    }
}
