//! 入站访问控制:CIDR 白名单 + 环回判定。对齐 Swift `ClientAccessControl`。

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
}
