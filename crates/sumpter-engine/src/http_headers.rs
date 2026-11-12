//! HTTP connection-scoped fields must never cross a proxy hop.
use std::collections::HashSet;

/// Cookie scope does not include the TCP port. Never send the Admin session
/// to a Provider when the browser also uses the data listener on that host.
pub fn without_admin_cookie(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers.iter().filter_map(|(name, value)| {
        if !name.eq_ignore_ascii_case("cookie") {
            return Some((name.clone(), value.clone()));
        }
        let mut kept = Vec::new();
        for pair in value.split(';') {
            let pair = pair.trim();
            if pair.is_empty() { continue; }
            let (key, val) = pair.split_once('=')?;
            let cookie_value = if val.starts_with('"') && val.ends_with('"') && val.len() >= 2 { &val[1..val.len()-1] } else { val };
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || !cookie_value.bytes().all(|b| matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)) {
                return None;
            }
            if key != "sumpter_admin_session" { kept.push(pair); }
        }
        (!kept.is_empty()).then(|| (name.clone(), kept.join("; ")))
    }).collect()
}

pub fn is_admin_path(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let Some(a) = bytes.next().and_then(|b| (b as char).to_digit(16)) else {
                return false;
            };
            let Some(b) = bytes.next().and_then(|b| (b as char).to_digit(16)) else {
                return false;
            };
            decoded.push((a * 16 + b) as u8);
        } else {
            decoded.push(byte);
        }
    }
    decoded == b"/admin" || decoded.starts_with(b"/admin/")
}

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
    fn removes_admin_cookie_from_every_header_and_drops_ambiguous_values() {
        let headers = without_admin_cookie(&[
            (
                "Cookie".into(),
                "a=1; sumpter_admin_session=secret; b=two".into(),
            ),
            ("cookie".into(), "sumpter_admin_session=again".into()),
            ("COOKIE".into(), "c=\"three\"".into()),
            (
                "Cookie".into(),
                "broken; sumpter_admin_session=secret".into(),
            ),
            ("Cookie".into(), "x=\"unterminated".into()),
        ]);
        assert_eq!(
            headers,
            vec![
                ("Cookie".into(), "a=1; b=two".into()),
                ("COOKIE".into(), "c=\"three\"".into())
            ]
        );
        for path in [
            "/admin",
            "/admin/api/config",
            "/%61dmin",
            "/admin%2Fapi?a=1",
        ] {
            assert!(is_admin_path(path));
        }
        for path in ["/administrator", "/v1/messages", "/v1/admin"] {
            assert!(!is_admin_path(path));
        }
    }
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
