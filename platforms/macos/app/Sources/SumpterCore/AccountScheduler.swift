import Foundation

/// 账号级的会话粘性 + 健康感知排序(纯逻辑,可单测)。
///
/// 相对无状态哈希 home(粘固定起点,可能恰好是坏账号),这里让"最近成功的账号"
/// 被同一会话粘住;近期连不上的账号临时沉底,直到冷却结束或再次成功。
/// 上层(ProxyEngine)负责记录成功/失败并维护 health 与会话→账号映射。
public enum AccountScheduler {
    public struct Health: Equatable, Sendable {
        public var coolingUntil: Date?   // 晚于 now 表示冷却中(近期连不上)
        public var lastSuccess: Date?    // 最近一次成功时间

        public init(coolingUntil: Date? = nil, lastSuccess: Date? = nil) {
            self.coolingUntil = coolingUntil
            self.lastSuccess = lastSuccess
        }
    }

    /// 账号候选按"会话粘性 + 健康"重排。
    /// - groupIDs: 账号 label,已按配置顺序去重。
    /// - stickyPreferred: 该会话最近成功的账号 label(优先粘住,除非它正冷却)。
    /// - homeIndex: 无粘性记录时的哈希起点(回退到原来的多账号分摊行为)。
    public static func ordered(
        groupIDs: [String],
        health: [String: Health],
        stickyPreferred: String?,
        homeIndex: Int,
        now: Date
    ) -> [String] {
        guard groupIDs.count > 1 else {
            return groupIDs
        }
        let start = ((homeIndex % groupIDs.count) + groupIDs.count) % groupIDs.count
        let rotated = Array(groupIDs[start...]) + Array(groupIDs[..<start])

        var preferred: [String] = []   // 会话粘住的成功账号(未冷却)
        var healthy: [String] = []     // 其余未冷却,保持 home 旋转顺序
        var cooling: [String] = []     // 冷却中(近期连不上),沉底兜底

        for id in rotated {
            let isCooling = (health[id]?.coolingUntil).map { $0 > now } ?? false
            if id == stickyPreferred && !isCooling {
                preferred.append(id)
            } else if isCooling {
                cooling.append(id)
            } else {
                healthy.append(id)
            }
        }
        return preferred + healthy + cooling
    }
}
