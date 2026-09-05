//! 会话归属 CAS 与粘性淘汰规则（纯逻辑，时间一律用 Unix 秒 f64）。

/// 并发请求下更新已归属入口的 CAS 规则：
/// - 首次成功可建立归属；同入口成功无需改写；
/// - 只有当前归属仍等于本请求开始时的入口，才允许把故障转移后的成功入口写回；
/// - 配置已删除的旧分组可被新成功入口替换；
/// - 其它情况说明另一请求已迁移归属，旧请求不得覆盖。
pub fn should_replace_sticky_assignment(
    current: Option<&str>,
    successful: &str,
    initial: &str,
    eligible_groups: &[String],
) -> bool {
    match current {
        None => true,
        Some(current) if current == successful => false,
        Some(current) if current == initial => true,
        Some(current) => !eligible_groups.iter().any(|group| group == current),
    }
}

/// 会话粘性归属的淘汰决策(纯函数,返回待删除的键)。
///
/// 两级:先淘汰 `at` 早于 `now - ttl_seconds` 的过期条目(**不论是否持久**——稳定会话的归属
/// 也必须能过期,否则文件只增不减);仍超过 `max_entries` 时按 `at` 最旧优先继续淘汰,同一
/// 时刻下非持久条目先走(持久归属更值得保留)。
///
/// `ttl_seconds <= 0` 表示不按 TTL 淘汰。键序在同 `(at, persistent)` 下按键名排序以保证确定性。
pub fn session_sticky_evictions(
    entries: &[(String, f64, bool)],
    now: f64,
    ttl_seconds: f64,
    max_entries: usize,
) -> Vec<String> {
    let mut expired: Vec<String> = Vec::new();
    let mut alive: Vec<&(String, f64, bool)> = Vec::with_capacity(entries.len());
    for entry in entries {
        // at 非有限(损坏值)视为过期,避免排序里出现 NaN 传染。
        let stale = ttl_seconds > 0.0 && (now - entry.1) > ttl_seconds;
        if stale || !entry.1.is_finite() {
            expired.push(entry.0.clone());
        } else {
            alive.push(entry);
        }
    }
    if alive.len() <= max_entries {
        return expired;
    }
    // 最旧优先;同刻先弃非持久;再按键名稳定收尾。
    alive.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    let excess = alive.len() - max_entries;
    expired.extend(alive.into_iter().take(excess).map(|entry| entry.0.clone()));
    expired
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn concurrent_old_success_cannot_overwrite_migrated_assignment() {
        let eligible = ids(&["a", "b"]);
        assert!(should_replace_sticky_assignment(None, "a", "a", &eligible));
        assert!(should_replace_sticky_assignment(
            Some("a"),
            "b",
            "a",
            &eligible
        ));
        assert!(!should_replace_sticky_assignment(
            Some("b"),
            "a",
            "a",
            &eligible
        ));
        assert!(!should_replace_sticky_assignment(
            Some("b"),
            "b",
            "b",
            &eligible
        ));
        assert!(should_replace_sticky_assignment(
            Some("deleted"),
            "a",
            "a",
            &eligible
        ));
    }

    fn entry(key: &str, at: f64, persistent: bool) -> (String, f64, bool) {
        (key.to_string(), at, persistent)
    }

    #[test]
    fn ttl_expires_persistent_assignments_too() {
        let entries = vec![
            entry("old-persistent", 0.0, true),
            entry("old-temp", 10.0, false),
            entry("fresh", 950.0, true),
        ];
        let mut evicted = session_sticky_evictions(&entries, 1_000.0, 100.0, 100);
        evicted.sort();
        assert_eq!(evicted, vec!["old-persistent", "old-temp"]);
    }

    #[test]
    fn zero_ttl_disables_expiry() {
        let entries = vec![entry("ancient", 0.0, true)];
        assert!(session_sticky_evictions(&entries, 1e9, 0.0, 100).is_empty());
    }

    #[test]
    fn over_cap_evicts_persistent_when_no_temp_left() {
        // 全部持久且都在 TTL 内:上限仍必须生效,否则 map 永久超限。
        let entries = vec![
            entry("a", 1.0, true),
            entry("b", 2.0, true),
            entry("c", 3.0, true),
        ];
        assert_eq!(
            session_sticky_evictions(&entries, 10.0, 0.0, 1),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn over_cap_prefers_temp_at_same_timestamp() {
        let entries = vec![
            entry("keep-persistent", 5.0, true),
            entry("temp", 5.0, false),
        ];
        assert_eq!(
            session_sticky_evictions(&entries, 10.0, 0.0, 1),
            vec!["temp".to_string()]
        );
    }

    #[test]
    fn non_finite_timestamp_is_evicted() {
        let entries = vec![entry("broken", f64::NAN, true), entry("ok", 5.0, true)];
        assert_eq!(
            session_sticky_evictions(&entries, 10.0, 0.0, 100),
            vec!["broken".to_string()]
        );
    }

    #[test]
    fn under_cap_within_ttl_evicts_nothing() {
        let entries = vec![entry("a", 9.0, true), entry("b", 9.5, false)];
        assert!(session_sticky_evictions(&entries, 10.0, 100.0, 2).is_empty());
    }
}
