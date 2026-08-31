import Foundation

/// 代理的即时健康态,按最近 N 条客户端请求(而非累计)判定,供菜单栏和运行页一眼观察。
public enum ProxyHealth: String, Sendable, Equatable {
    case stopped    // 未运行
    case idle       // 运行中但近窗无请求
    case healthy    // 近窗成功率高
    case degraded   // 有成功也有失败
    case down       // 近窗全部失败
}

public struct ProxyHealthSummary: Sendable, Equatable {
    public var state: ProxyHealth
    public var considered: Int          // 参与判定的请求数(不含取消)
    public var successes: Int
    public var successRate: Double?     // considered==0 时为 nil
    public var lastSuccess: Date?       // 最近一次成功的时间;由展示方算「多久前」
    public var headline: String         // 中文一行摘要(菜单栏 tooltip 用)

    public init(
        state: ProxyHealth,
        considered: Int = 0,
        successes: Int = 0,
        successRate: Double? = nil,
        lastSuccess: Date? = nil,
        headline: String
    ) {
        self.state = state
        self.considered = considered
        self.successes = successes
        self.successRate = successRate
        self.lastSuccess = lastSuccess
        self.headline = headline
    }
}

public enum ProxyHealthEvaluator {
    /// 健康率达到该阈值算「正常」,否则「降级」。
    public static let healthyThreshold = 0.8

    /// 从运行事件推导即时健康。`events` 是新到旧(引擎在头部插入)。
    /// - window: 参与判定的最近客户端请求条数。
    public static func evaluate(
        events: [RuntimeEvent],
        isRunning: Bool,
        window: Int = 20
    ) -> ProxyHealthSummary {
        guard isRunning else {
            return ProxyHealthSummary(state: .stopped, headline: "已停止")
        }

        // 进行中的事件还没有最终状态,不参与健康判定(否则长流式期间会误报降级)。
        let clientEvents = events.filter { $0.kind == "client" && !$0.isInFlight }
        // 取消既不算成功也不算失败，先排除再截窗口，和 Rust 健康评估保持一致。
        let graded = Array(clientEvents.lazy.filter { !$0.isCancelled }.prefix(max(1, window)))
        let lastSuccess = clientEvents.first(where: \.isSucceeded)?.timestamp

        guard !graded.isEmpty else {
            return ProxyHealthSummary(
                state: .idle,
                lastSuccess: lastSuccess,
                headline: clientEvents.isEmpty ? "运行中 · 暂无请求" : "运行中 · 近期无有效请求"
            )
        }

        let successes = graded.filter(\.isSucceeded).count
        let rate = Double(successes) / Double(graded.count)
        let percent = Int((rate * 100).rounded())

        let state: ProxyHealth
        let headline: String
        if successes == 0 {
            state = .down
            headline = "上游全部失败 · 近 \(graded.count) 请求 0% 成功"
        } else if rate >= healthyThreshold {
            state = .healthy
            headline = "正常 · 近 \(graded.count) 请求 \(percent)% 成功"
        } else {
            state = .degraded
            headline = "降级 · 近 \(graded.count) 请求 \(percent)% 成功"
        }

        return ProxyHealthSummary(
            state: state,
            considered: graded.count,
            successes: successes,
            successRate: rate,
            lastSuccess: lastSuccess,
            headline: headline
        )
    }
}
