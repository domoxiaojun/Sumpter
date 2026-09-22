//! 入站访问控制:CIDR 白名单 + 环回判定。对齐 Swift `ClientAccessControl`。
//! 另含「转发头 → 客户端 IP」解析:只影响事件 `sourceIP`,不参与鉴权。

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

/// 已知代理匹配:只用于 `X-Forwarded-For` 链的跳过判断,不是读取转发头的前置条件。
/// 空列表不匹配任何地址,环回不自动匹配,非法条目忽略——与 [`is_allowed`] 的
/// 「空列表允许所有、环回恒放行」语义刻意不同。
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

/// 由请求头与 TCP 对端解析事件应记录的客户端 IP。
///
/// 规则(HTTP 与 WebSocket 共用),按顺序取第一个合法值:
/// 1. `X-Real-IP`:必须是单个纯 IP(IPv6 允许 `[...]` 包裹)。
/// 2. `X-Forwarded-For`:合并同名头(按出现顺序,逗号连接),每一项都必须是纯 IP;
///    `trusted_proxy_cidrs` 为空时取最左侧(约定的原始客户端),非空时从右向左
///    跳过所列已知代理,取第一个非代理地址,全部是代理则取最左侧。
/// 3. 两个头都缺失或不合法(空、主机名、端口、超过 4 KiB 或 32 项)→ TCP 对端;
///    对端也未知 → `None`。
///
/// 转发头由请求方声明,这里不校验其来源:结果只用于事件 `sourceIP`,鉴权、
/// `allowedCIDRs` 与 `/__status` 继续只看 TCP 对端。IPv4-mapped IPv6 折叠为 IPv4,
/// 返回值已是规范文本形式。
pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    headers: &[(String, String)],
    trusted_proxy_cidrs: &[String],
) -> Option<IpAddr> {
    if let Some(client) =
        merged_header(headers, "x-real-ip").and_then(|raw| single_forwarded_ip(&raw))
    {
        return Some(client);
    }
    if let Some(client) = merged_header(headers, "x-forwarded-for")
        .and_then(|raw| client_from_forwarded_chain(&raw, trusted_proxy_cidrs))
    {
        return Some(client);
    }
    peer.map(fold_v4_mapped)
}

/// 便于事件层直接落成字符串。
pub fn resolve_client_ip_text(
    peer: Option<IpAddr>,
    headers: &[(String, String)],
    trusted_proxy_cidrs: &[String],
) -> Option<String> {
    resolve_client_ip(peer, headers, trusted_proxy_cidrs).map(|ip| ip.to_string())
}

/// 合并同名头;不存在返回 `None`,存在但为空返回 `Some("")`(随后按不合法处理,落到下一来源)。
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
    if trusted_proxy_cidrs.is_empty() {
        // 没有登记代理:按约定最左侧是原始客户端。
        return chain.first().copied();
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
    fn forwarded_headers_are_honored_without_a_trust_list() {
        let declared = [
            ("x-forwarded-for", "198.51.100.99"),
            ("x-real-ip", "198.51.100.99"),
        ];
        // 空列表、不在列表内的对端、未知对端:只要头合法就采信。
        assert_eq!(
            resolved("127.0.0.1", &declared, &[]).as_deref(),
            Some("198.51.100.99")
        );
        assert_eq!(
            resolved("203.0.113.7", &declared, &["10.0.0.0/8"]).as_deref(),
            Some("198.51.100.99")
        );
        assert_eq!(
            resolve_client_ip_text(None, &headers(&declared), &[]).as_deref(),
            Some("198.51.100.99")
        );
        // 没有任何转发头:记录对端;对端也未知则为空。
        assert_eq!(
            resolved("127.0.0.1", &[], &[]).as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(resolve_client_ip(None, &[], &[]), None);
        assert_eq!(resolve_client_ip(None, &[], &cidrs(&["0.0.0.0/0"])), None);
    }

    #[test]
    fn real_ip_wins_then_forwarded_for_then_peer() {
        let both = [
            ("x-forwarded-for", "198.51.100.99"),
            ("x-real-ip", "192.0.2.10"),
        ];
        assert_eq!(
            resolved("10.0.0.2", &both, &[]).as_deref(),
            Some("192.0.2.10")
        );
        // 登记了代理也不改变优先级。
        assert_eq!(
            resolved("10.0.0.2", &both, &["10.0.0.0/8"]).as_deref(),
            Some("192.0.2.10")
        );
        assert_eq!(
            resolved("10.0.0.2", &[("x-forwarded-for", "198.51.100.99")], &[]).as_deref(),
            Some("198.51.100.99")
        );
        // 头名大小写不敏感;本机反代的环回对端也不需要登记。
        assert_eq!(
            resolved("127.0.0.1", &[("X-Real-IP", "198.51.100.5")], &[]).as_deref(),
            Some("198.51.100.5")
        );
    }

    #[test]
    fn forwarded_chain_takes_leftmost_unless_known_proxies_are_listed() {
        // 没有登记代理:约定最左侧是原始客户端。
        assert_eq!(
            resolved(
                "172.16.0.9",
                &[("x-forwarded-for", "198.51.100.99, 10.0.0.1")],
                &[]
            )
            .as_deref(),
            Some("198.51.100.99")
        );
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
        // 客户端伪造前缀:登记了代理时取最右侧非代理项。
        assert_eq!(
            resolved(
                "10.0.0.1",
                &[("x-forwarded-for", "1.2.3.4, 198.51.100.99")],
                &trusted
            )
            .as_deref(),
            Some("198.51.100.99")
        );
        // 全部是已知代理:取最左侧。
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
    fn malformed_headers_fall_through_to_the_next_source() {
        // X-Real-IP 不合法 → 试 X-Forwarded-For → 再回退对端。
        for bad in [
            "",
            " ",
            "unknown",
            "192.0.2.10:8080",
            "192.0.2.10, 192.0.2.11",
            "host.example",
            "[::1",
            "::1]",
            "for=192.0.2.10",
        ] {
            assert_eq!(
                resolved(
                    "10.0.0.1",
                    &[("x-real-ip", bad), ("x-forwarded-for", "198.51.100.99")],
                    &[]
                )
                .as_deref(),
                Some("198.51.100.99"),
                "{bad:?}"
            );
            assert_eq!(
                resolved("10.0.0.1", &[("x-real-ip", bad)], &[]).as_deref(),
                Some("10.0.0.1"),
                "{bad:?}"
            );
        }
        // X-Forwarded-For 链里任一项不合法,整条链作废。
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
                resolved("10.0.0.1", &[("x-forwarded-for", bad)], &[]).as_deref(),
                Some("10.0.0.1"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn forwarded_chain_limits_length_and_entries() {
        let too_many = std::iter::repeat_n("198.51.100.1", MAX_FORWARDED_CHAIN_ENTRIES + 1)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            resolved("10.0.0.1", &[("x-forwarded-for", &too_many)], &[]).as_deref(),
            Some("10.0.0.1")
        );
        let at_limit = std::iter::repeat_n("198.51.100.1", MAX_FORWARDED_CHAIN_ENTRIES)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            resolved("10.0.0.1", &[("x-forwarded-for", &at_limit)], &[]).as_deref(),
            Some("198.51.100.1")
        );
        let too_long = format!("{}198.51.100.1", " ".repeat(MAX_FORWARDED_HEADER_BYTES));
        assert_eq!(
            resolved("10.0.0.1", &[("x-forwarded-for", &too_long)], &[]).as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(
            resolved("10.0.0.1", &[("x-real-ip", &too_long)], &[]).as_deref(),
            Some("10.0.0.1")
        );
    }

    #[test]
    fn forwarded_addresses_support_ipv6_and_fold_v4_mapped() {
        assert_eq!(
            resolved(
                "fd00::1",
                &[("x-forwarded-for", "2001:db8::42, [fd00::2]")],
                &["fd00::/8"]
            )
            .as_deref(),
            Some("2001:db8::42")
        );
        assert_eq!(
            resolved(
                "::ffff:10.0.0.1",
                &[("x-forwarded-for", "::ffff:198.51.100.99")],
                &[]
            )
            .as_deref(),
            Some("198.51.100.99")
        );
        assert_eq!(
            resolved("::ffff:10.0.0.1", &[], &[]).as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(
            resolved("10.0.0.1", &[("x-real-ip", "[2001:db8::7]")], &[]).as_deref(),
            Some("2001:db8::7")
        );
        assert_eq!(
            resolved("10.0.0.1", &[("x-real-ip", "::ffff:192.0.2.10")], &[]).as_deref(),
            Some("192.0.2.10")
        );
    }
}
