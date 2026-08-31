import Foundation

/// pinned IP 候选的健康感知排序(纯逻辑,可单测)。
///
/// 针对"时好时坏"的 pinned IP:优先近期成功的、跳过冷却中的(刚失败的),
/// 未试过的用滚动偏移分散,冷却中的排到最后作为兜底。上层再按 ip_concurrency 分批赛跑。
public enum PinnedIPScheduler {
    public struct Health: Equatable, Sendable {
        public var coolingUntil: Date?   // 设置且晚于 now 表示该 IP 冷却中(刚失败)
        public var lastSuccess: Date?    // 最近一次成功时间

        public init(coolingUntil: Date? = nil, lastSuccess: Date? = nil) {
            self.coolingUntil = coolingUntil
            self.lastSuccess = lastSuccess
        }
    }

    /// 按健康度重排候选。`nil`(DNS 兜底)始终视为可用、不冷却。
    /// - rotation: 滚动计数,用于在同档候选间分散首选,避免每次都先撞同一个。
    public static func ordered(
        candidates: [String?],
        health: [String: Health],
        now: Date,
        rotation: Int
    ) -> [String?] {
        guard candidates.count > 1 else {
            return candidates
        }

        var succeeded: [(ip: String?, at: Date)] = []  // 近期成功且未冷却:最可能仍然好
        var untried: [String?] = []                    // 可用但无成功记录
        var cooling: [String?] = []                    // 冷却中(刚失败),兜底

        for candidate in candidates {
            guard let ip = candidate else {
                untried.append(candidate)   // DNS 兜底:当作可用
                continue
            }
            let entry = health[ip]
            if let until = entry?.coolingUntil, until > now {
                cooling.append(candidate)
            } else if let last = entry?.lastSuccess {
                succeeded.append((candidate, last))
            } else {
                untried.append(candidate)
            }
        }

        // 近期成功的按时间倒序(最新成功的先试);未试/冷却的做滚动分散。
        let preferred = succeeded.sorted { $0.at > $1.at }.map(\.ip)
        return preferred + rotate(untried, by: rotation) + rotate(cooling, by: rotation)
    }

    private static func rotate(_ items: [String?], by rotation: Int) -> [String?] {
        guard items.count > 1 else {
            return items
        }
        let offset = ((rotation % items.count) + items.count) % items.count
        return Array(items[offset...]) + Array(items[..<offset])
    }
}
