//! 入站访问控制:CIDR 白名单 + 环回判定。对齐 Swift `ClientAccessControl`。
//! 另含「可信代理 → 真实客户端 IP」解析:只影响事件 `sourceIP`,不参与鉴权。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 判定顺序(对齐 Swift):
/// 1. host 缺失/解析失败 → 仅当 allowedCIDRs 为空才放行;
/// 2. 环回恒放行;
/// 3. allowedCIDRs 为空 → 放行;
/// 4. 否则任一 CIDR 包含即放行。
pub fn is_allowed(client_host: Option<&str>, allowed_cidrs: &[String]) -> bool {
    let Some(address) = client_host.and_then(parse_ip) else {
        return allowed_cidrs.is_empty();
    };
    if ip_is_loopback(&address) {
        return true;
    }
    if allowed_cidrs.is_empty() {
        return true;
    }
    allowed_cidrs
        .iter()
        .any(|cidr| CidrNetwork::parse(cidr).is_some_and(|n| n.contains(&address)))
}

/// 客户端是否来自环回(127.0.0.0/8 或 ::1);缺失/解析失败视为非环回。
pub fn is_loopback(client_host: Option<&str>) -> bool {
    client_host
        .and_then(parse_ip)
        .is_some_and(|a| ip_is_loopback(&a))
}

/// CIDR / 裸 IP 字面量是否合法(供配置加载与两端保存前校验)。
pub fn is_valid_cidr(raw: &str) -> bool {
    CidrNetwork::parse(raw).is_some()
}

/// 严格的可信代理匹配:空列表不信任任何人,环回不自动放行,非法条目忽略。
/// 与 [`is_allowed`] 的「空列表允许所有、环回恒放行」语义刻意不同——
/// 信任转发头的默认必须是关闭。
pub fn is_trusted_proxy(peer: &IpAddr, trusted_proxy_cidrs: &[String]) -> bool {
    let peer = fold_v4_mapped(*peer);
    trusted_proxy_cidrs
        .iter()
        .any(|cidr| CidrNetwork::parse(cidr).is_some_and(|n| n.contains(&peer)))
}

/// 单个转发头值允许的最大字节数(合并同名头后计)。
pub const MAX_FORWARDED_HEADER_BYTES: usize = 4 * 1024;
/// `X-Forwarded-For` 地址链允许的最大项数。
pub const MAX_FORWARDED_CHAIN_ENTRIES: usize = 32;

/// 由 TCP 对端与请求头解析事件应记录的客户端 IP。
///
/// 规则(HTTP 与 WebSocket 共用):
/// 1. 对端未知 → `None`;对端不在可信代理列表 → 直接返回对端。
/// 2. 对端可信时优先 `X-Forwarded-For`:合并同名头(按出现顺序,逗号连接),
///    从右向左跳过可信代理,取第一个非可信地址;全部可信则取最左侧。
/// 3. 仅当 `X-Forwarded-For` 不存在时使用单值 `X-Real-IP`。
/// 4. 选中的头为空、非 IP、带端口/主机名、超过 4 KiB 或 32 项 → 回退对端,
///    不再尝试另一种头。
/// 5. IPv4-mapped IPv6 折叠为 IPv4;返回值已是规范文本形式。
pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    headers: &[(String, String)],
    trusted_proxy_cidrs: &[String],
) -> Option<IpAddr> {
    let peer = fold_v4_mapped(peer?);
    if trusted_proxy_cidrs.is_empty() || !is_trusted_proxy(&peer, trusted_proxy_cidrs) {
        return Some(peer);
    }
    let forwarded_for = merged_header(headers, "x-forwarded-for");
    if let Some(raw) = forwarded_for {
        return Some(client_from_forwarded_chain(&raw, trusted_proxy_cidrs).unwrap_or(peer));
    }
    if let Some(raw) = merged_header(headers, "x-real-ip") {
        return Some(single_forwarded_ip(&raw).unwrap_or(peer));
    }
    Some(peer)
}

/// 便于事件层直接落成字符串。
pub fn resolve_client_ip_text(
    peer: Option<IpAddr>,
    headers: &[(String, String)],
    trusted_proxy_cidrs: &[String],
) -> Option<String> {
    resolve_client_ip(peer, headers, trusted_proxy_cidrs).map(|ip| ip.to_string())
}

/// 合并同名头;不存在返回 `None`(存在但为空返回 `Some("")`,以便按「选中即不再回退到另一种头」处理)。
fn merged_header(headers: &[(String, String)], name: &str) -> Option<String> {
    let mut merged: Option<String> = None;
    for (key, value) in headers {
        if !key.eq_ignore_ascii_case(name) {
            continue;
        }
        match &mut merged {
            Some(existing) => {
                existing.push(',');
                existing.push_str(value);
            }
            None => merged = Some(value.clone()),
        }
    }
    merged
}

fn client_from_forwarded_chain(raw: &str, trusted_proxy_cidrs: &[String]) -> Option<IpAddr> {
    if raw.len() > MAX_FORWARDED_HEADER_BYTES {
        return None;
    }
    let mut chain = Vec::new();
    for entry in raw.split(',') {
        if chain.len() >= MAX_FORWARDED_CHAIN_ENTRIES {
            return None;
        }
        chain.push(strict_ip(entry)?);
    }
    if chain.is_empty() {
        return None;
    }
    chain
        .iter()
        .rev()
        .find(|address| !is_trusted_proxy(address, trusted_proxy_cidrs))
        .or_else(|| chain.first())
        .copied()
}

fn single_forwarded_ip(raw: &str) -> Option<IpAddr> {
    if raw.len() > MAX_FORWARDED_HEADER_BYTES || raw.contains(',') {
        return None;
    }
    strict_ip(raw)
}

/// 转发头里的地址必须是纯 IP:不接受主机名、端口、`[...]` 以外的装饰。
/// IPv6 允许 `[::1]` 写法,IPv4-mapped 折叠。
fn strict_ip(raw: &str) -> Option<IpAddr> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unwrapped = match (trimmed.starts_with('['), trimmed.ends_with(']')) {
        (true, true) => &trimmed[1..trimmed.len() - 1],
        (false, false) => trimmed,
        _ => return None,
    };
    let addr: IpAddr = unwrapped.parse().ok()?;
    Some(fold_v4_mapped(addr))
}

/// 解析 IP(容忍 `[...]` 包裹与两端空白);IPv4-mapped IPv6 折叠为 IPv4。
fn parse_ip(raw: &str) -> Option<IpAddr> {
    let trimmed = raw.trim().trim_matches(|c| c == '[' || c == ']');
    let addr: IpAddr = trimmed.parse().ok()?;
    Some(fold_v4_mapped(addr))
}

fn fold_v4_mapped(addr: IpAddr) -> IpAddr {
    if let IpAddr::V6(v6) = addr
        && let Some(v4) = v6.to_ipv4_mapped()
    {
        return IpAddr::V4(v4);
    }
    addr
}

fn ip_is_loopback(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4.octets()[0] == 127,
        IpAddr::V6(v6) => *v6 == Ipv6Addr::LOCALHOST,
    }
}

struct CidrNetwork {
    address: IpAddr,
    prefix_length: u32,
}

impl CidrNetwork {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let (addr_part, prefix_part) = match trimmed.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (trimmed, None),
        };
        let address = parse_ip(addr_part)?;
        let bits = ip_bits(&address);
        let prefix_length = match prefix_part {
            Some(p) => {
                let value: u32 = p.parse().ok()?;
                if value > bits {
                    return None;
                }
                value
            }
            None => bits,
        };
        Some(Self {
            address,
            prefix_length,
        })
    }

    fn contains(&self, candidate: &IpAddr) -> bool {
        if ip_bits(&self.address) != ip_bits(candidate) {
            return false;
        }
        let network = ip_octets(&self.address);
        let target = ip_octets(candidate);
        let full_bytes = (self.prefix_length / 8) as usize;
        let remaining_bits = self.prefix_length % 8;

        if network[..full_bytes] != target[..full_bytes] {
            return false;
        }
        if remaining_bits == 0 {
            return true;
        }
        let mask = 0xffu8 << (8 - remaining_bits);
        (network[full_bytes] & mask) == (target[full_bytes] & mask)
    }
}

fn ip_bits(addr: &IpAddr) -> u32 {
    match addr {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

fn ip_octets(addr: &IpAddr) -> Vec<u8> {
    match addr {
        IpAddr::V4(v4) => v4.octets().to_vec(),
        IpAddr::V6(v6) => v6.octets().to_vec(),
    }
}

#[allow(dead_code)]
fn _types(_: Ipv4Addr) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn cidrs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn loopback_always_allowed() {
        assert!(is_allowed(Some("127.0.0.1"), &cidrs(&["10.0.0.0/8"])));
        assert!(is_allowed(Some("127.9.9.9"), &cidrs(&["10.0.0.0/8"])));
        assert!(is_allowed(Some("::1"), &cidrs(&["10.0.0.0/8"])));
        assert!(is_allowed(Some("[::1]"), &cidrs(&["10.0.0.0/8"])));
        assert!(is_loopback(Some("127.0.0.1")));
        assert!(is_loopback(Some("::ffff:127.0.0.1")));
        assert!(!is_loopback(Some("192.168.1.5")));
        assert!(!is_loopback(None));
        assert!(!is_loopback(Some("garbage")));
    }

    #[test]
    fn empty_cidrs_allow_everyone() {
        assert!(is_allowed(Some("8.8.8.8"), &[]));
        assert!(is_allowed(None, &[]));
        assert!(is_allowed(Some("not-an-ip"), &[]));
    }

    #[test]
    fn cidr_matching() {
        let allowed = cidrs(&["192.168.1.0/24", "10.0.0.0/8"]);
        assert!(is_allowed(Some("192.168.1.42"), &allowed));
        assert!(is_allowed(Some("10.200.3.4"), &allowed));
        assert!(!is_allowed(Some("192.168.2.1"), &allowed));
        assert!(!is_allowed(Some("8.8.8.8"), &allowed));
        // 解析不出的 host 在有白名单时拒绝。
        assert!(!is_allowed(Some("garbage"), &allowed));
        assert!(!is_allowed(None, &allowed));
    }

    #[test]
    fn v4_mapped_v6_and_exact_host_cidr() {
        // ::ffff:192.168.1.5 折叠成 v4 后可命中 v4 CIDR(Swift 同款折叠)。
        assert!(is_allowed(
            Some("::ffff:192.168.1.5"),
            &cidrs(&["192.168.1.0/24"])
        ));
        // 无 / 的裸地址 = 全前缀精确匹配。
        assert!(is_allowed(Some("1.2.3.4"), &cidrs(&["1.2.3.4"])));
        assert!(!is_allowed(Some("1.2.3.5"), &cidrs(&["1.2.3.4"])));
        // 非法 CIDR 条目被忽略。
        assert!(!is_allowed(Some("1.2.3.4"), &cidrs(&["1.2.3.4/33"])));
        // 不同地址族不匹配。
        assert!(!is_allowed(Some("fe80::1"), &cidrs(&["10.0.0.0/8"])));
    }

    #[test]
    fn bit_boundary_masks() {
        assert!(is_allowed(Some("10.0.0.129"), &cidrs(&["10.0.0.128/25"])));
        assert!(!is_allowed(Some("10.0.0.127"), &cidrs(&["10.0.0.128/25"])));
        assert!(is_allowed(Some("2001:db8::1"), &cidrs(&["2001:db8::/32"])));
        assert!(!is_allowed(Some("2001:db9::1"), &cidrs(&["2001:db8::/32"])));
    }

    #[test]
    fn cidr_literal_validation() {
        assert!(is_valid_cidr("127.0.0.1"));
        assert!(is_valid_cidr("10.0.0.0/8"));
        assert!(is_valid_cidr("::1/128"));
        assert!(is_valid_cidr(" [::1] "));
        assert!(!is_valid_cidr("10.0.0.0/99"));
        assert!(!is_valid_cidr("not-an-ip"));
        assert!(!is_valid_cidr(""));
        assert!(!is_valid_cidr("10.0.0.1:8080"));
    }

    fn peer(text: &str) -> Option<IpAddr> {
        Some(text.parse().unwrap())
    }

    fn headers(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn resolved(peer_text: &str, list: &[(&str, &str)], trusted: &[&str]) -> Option<String> {
        resolve_client_ip_text(peer(peer_text), &headers(list), &cidrs(trusted))
    }

    #[test]
    fn trusted_proxy_matching_is_strict() {
        // 空列表不信任任何人,包括环回;非法条目被忽略而不是放行。
        assert!(!is_trusted_proxy(&"127.0.0.1".parse().unwrap(), &[]));
        assert!(!is_trusted_proxy(
            &"127.0.0.1".parse().unwrap(),
            &cidrs(&["10.0.0.0/8"])
        ));
        assert!(!is_trusted_proxy(
            &"10.1.2.3".parse().unwrap(),
            &cidrs(&["10.0.0.0/99"])
        ));
        assert!(is_trusted_proxy(
            &"10.1.2.3".parse().unwrap(),
            &cidrs(&["10.0.0.0/8"])
        ));
        assert!(is_trusted_proxy(
            &"::ffff:10.1.2.3".parse().unwrap(),
            &cidrs(&["10.0.0.0/8"])
        ));
        assert!(is_trusted_proxy(
            &"127.0.0.1".parse().unwrap(),
            &cidrs(&["127.0.0.1"])
        ));
    }

    #[test]
    fn empty_trust_list_and_untrusted_peer_keep_tcp_peer() {
        let spoof = [
            ("x-forwarded-for", "198.51.100.99"),
            ("x-real-ip", "198.51.100.99"),
        ];
        assert_eq!(
            resolved("127.0.0.1", &spoof, &[]).as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            resolved("203.0.113.7", &spoof, &["10.0.0.0/8"]).as_deref(),
            Some("203.0.113.7")
        );
        assert_eq!(
            resolve_client_ip(None, &headers(&spoof), &cidrs(&["0.0.0.0/0"])),
            None
        );
    }

    #[test]
    fn trusted_peer_uses_forwarded_for_before_real_ip() {
        let both = [
            ("x-real-ip", "192.0.2.10"),
            ("x-forwarded-for", "198.51.100.99"),
        ];
        assert_eq!(
            resolved("10.0.0.2", &both, &["10.0.0.0/8"]).as_deref(),
            Some("198.51.100.99")
        );
        assert_eq!(
            resolved("10.0.0.2", &[("x-real-ip", "192.0.2.10")], &["10.0.0.0/8"]).as_deref(),
            Some("192.0.2.10")
        );
        // 可信对端但没有任何转发头:仍是对端,不伪造。
        assert_eq!(
            resolved("10.0.0.2", &[], &["10.0.0.0/8"]).as_deref(),
            Some("10.0.0.2")
        );
        // 显式信任环回:本机反代也能传递。
        assert_eq!(
            resolved(
                "127.0.0.1",
                &[("x-forwarded-for", "198.51.100.5")],
                &["127.0.0.1"]
            )
            .as_deref(),
            Some("198.51.100.5")
        );
    }

    #[test]
    fn forwarded_chain_skips_trusted_hops_from_the_right() {
        let trusted = ["10.0.0.0/8", "172.16.0.0/12"];
        // 客户端 → 边界代理 10.0.0.1 → 内层代理 172.16.0.9 → Sumpter(对端 172.16.0.9)
        assert_eq!(
            resolved(
                "172.16.0.9",
                &[("x-forwarded-for", "198.51.100.99, 10.0.0.1")],
                &trusted
            )
            .as_deref(),
            Some("198.51.100.99")
        );
        // 客户端伪造前缀:取最右侧非可信项。
        assert_eq!(
            resolved(
                "10.0.0.1",
                &[("x-forwarded-for", "1.2.3.4, 198.51.100.99")],
                &trusted
            )
            .as_deref(),
            Some("198.51.100.99")
        );
        // 全部可信:取最左侧。
        assert_eq!(
            resolved(
                "10.0.0.1",
                &[("x-forwarded-for", "10.9.9.9, 172.16.0.1")],
                &trusted
            )
            .as_deref(),
            Some("10.9.9.9")
        );
        // 同名头按出现顺序合并。
        assert_eq!(
            resolved(
                "10.0.0.1",
                &[
                    ("x-forwarded-for", "198.51.100.99"),
                    ("X-Forwarded-For", "10.0.0.5")
                ],
                &trusted
            )
            .as_deref(),
            Some("198.51.100.99")
        );
    }

    #[test]
    fn malformed_forwarded_headers_fall_back_to_peer_without_trying_the_other_header() {
        let trusted = ["10.0.0.0/8"];
        for bad in [
            "",
            " ",
            "unknown",
            "198.51.100.99:8080",
            "198.51.100.99, evil.example",
            "198.51.100.99,",
            "[::1",
            "::1]",
            "for=198.51.100.99",
        ] {
            assert_eq!(
                resolved(
                    "10.0.0.1",
                    &[("x-forwarded-for", bad), ("x-real-ip", "192.0.2.10")],
                    &trusted
                )
                .as_deref(),
                Some("10.0.0.1"),
                "{bad:?}"
            );
        }
        // X-Real-IP 只接受单值。
        assert_eq!(
            resolved(
                "10.0.0.1",
                &[("x-real-ip", "192.0.2.10, 192.0.2.11")],
                &trusted
            )
            .as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(
            resolved("10.0.0.1", &[("x-real-ip", "host.example")], &trusted).as_deref(),
            Some("10.0.0.1")
        );
    }

    #[test]
    fn forwarded_chain_limits_length_and_entries() {
        let trusted = ["10.0.0.0/8"];
        let too_many = std::iter::repeat_n("198.51.100.1", MAX_FORWARDED_CHAIN_ENTRIES + 1)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            resolved("10.0.0.1", &[("x-forwarded-for", &too_many)], &trusted).as_deref(),
            Some("10.0.0.1")
        );
        let at_limit = std::iter::repeat_n("198.51.100.1", MAX_FORWARDED_CHAIN_ENTRIES)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            resolved("10.0.0.1", &[("x-forwarded-for", &at_limit)], &trusted).as_deref(),
            Some("198.51.100.1")
        );
        let too_long = format!("{}198.51.100.1", " ".repeat(MAX_FORWARDED_HEADER_BYTES));
        assert_eq!(
            resolved("10.0.0.1", &[("x-forwarded-for", &too_long)], &trusted).as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(
            resolved("10.0.0.1", &[("x-real-ip", &too_long)], &trusted).as_deref(),
            Some("10.0.0.1")
        );
    }

    #[test]
    fn forwarded_addresses_support_ipv6_and_fold_v4_mapped() {
        let trusted = ["10.0.0.0/8", "fd00::/8"];
        assert_eq!(
            resolved(
                "fd00::1",
                &[("x-forwarded-for", "2001:db8::42, [fd00::2]")],
                &trusted
            )
            .as_deref(),
            Some("2001:db8::42")
        );
        assert_eq!(
            resolved(
                "::ffff:10.0.0.1",
                &[("x-forwarded-for", "::ffff:198.51.100.99")],
                &trusted
            )
            .as_deref(),
            Some("198.51.100.99")
        );
        assert_eq!(
            resolved("::ffff:10.0.0.1", &[], &trusted).as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(
            resolved("10.0.0.1", &[("x-real-ip", "[2001:db8::7]")], &trusted).as_deref(),
            Some("2001:db8::7")
        );
    }
}
