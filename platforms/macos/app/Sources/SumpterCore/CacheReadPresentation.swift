import Foundation

public struct CacheReadStatistics: Codable, Equatable, Sendable {
    public var hitRequests: Int
    public var missRequests: Int
    public var unknownRequests: Int
    public var pendingRequests: Int
    public var notApplicableRequests: Int
    public var applicabilityUnknownRequests: Int
    public var confirmedHitRate: Double?
    public var confirmationCoverage: Double?
}

public struct CacheReadEvidence: Codable, Equatable, Sendable {
    public var finality: String
    public var observed: Bool
    public var complete: Bool
    public var truncated: Bool
    public var issue: String?
}

public struct CacheReadSummary: Codable, Equatable, Sendable {
    public var state: String
    public var readTokens: Int?
    public var finality: String
    public var reason: String?

    public var label: String {
        if ["hit", "miss"].contains(state), let readTokens, readTokens >= 0 {
            return "缓存读取 \(readTokens.formatted())"
        }
        return state == "not_applicable" ? "缓存读取 不适用" : "缓存读取 —"
    }

    public var statusLabel: String {
        ["hit": "已读取", "miss": "未读取", "pending": "等待上报", "unknown": "未知", "not_applicable": "不适用"][state] ?? "未知"
    }

    public var reasonLabel: String {
        guard let reason else { return "—" }
        return ["unreported": "上游未报告", "not_observed": "未观测到用量", "unsupported_transport": "此传输未采集用量",
                "unknown_applicability": "适用性未确定", "observation_truncated": "观测不完整", "invalid_value": "无效数值",
                "conflicting_evidence": "上游证据冲突", "insufficient_evidence": "证据不足"][reason] ?? reason
    }
}

public extension RuntimeEvent {
    var cacheReadLabel: String { cacheRead?.label ?? (kind == "notify" ? "缓存读取 不适用" : "缓存读取 —") }
    var observedUsage: ResponseUsage? { usageSummary ?? streamTrace?.usage }
    var hasObservedUsage: Bool {
        let usage = observedUsage
        return [usage?.inputTokens, usage?.outputTokens, usage?.cacheReadInputTokens,
                usage?.cacheCreationInputTokens, usage?.reasoningTokens, cacheRead?.readTokens]
            .contains { $0.map { $0 >= 0 } ?? false }
    }
    var cacheReadTokenRatio: Double? {
        guard let cacheRead, ["hit", "miss"].contains(cacheRead.state), cacheRead.finality == "confirmed",
              let read = cacheRead.readTokens, let usage = observedUsage, let input = usage.inputTokens else { return nil }
        let denominator: Double
        switch targetFormat ?? sourceFormat {
        case .anthropic:
            guard let write = usage.cacheCreationInputTokens else { return nil }
            denominator = Double(input) + Double(read) + Double(write)
        case .openai, .openaiResponses, .gemini: denominator = Double(input)
        default: return nil
        }
        guard denominator > 0, read >= 0, Double(read) <= denominator else { return nil }
        return Double(read) / denominator
    }
    var usageSummaryLabel: String {
        guard kind != "notify" else { return "不适用" }
        guard hasObservedUsage else { return isInFlight ? "等待用量" : "未报告用量" }
        let input = observedUsage?.inputTokens.flatMap { $0 >= 0 ? $0.formatted() : nil } ?? "—"
        let output = observedUsage?.outputTokens.flatMap { $0 >= 0 ? $0.formatted() : nil } ?? "—"
        return "输入 \(input) · 输出 \(output)"
    }
    var cacheReadHitRateLabel: String {
        guard let ratio = cacheReadTokenRatio else { return "命中率 —" }
        let value = (ratio * 100).formatted(.number.precision(.fractionLength(0...1)).locale(Locale(identifier: "zh_CN")))
        return "命中率 \(value)%"
    }
    var agentSummaryLabel: String? {
        let labels = ["root": "主代理", "subagent": "子代理", "guardian": "Guardian", "review": "审查", "memory": "记忆任务",
                      "title": "标题任务", "automation": "自动任务", "system": "系统", "ambient": "后台任务"]
        let parts = [agentName, agentRole.flatMap { labels[$0] }].compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// Pagination explicitly declares omitted detail fields. Full SSE events
    /// can clear them, whereas projections preserve the already-loaded details.
    func mergingProjection(_ incoming: RuntimeEvent) -> RuntimeEvent {
        guard incoming.id == id else { return incoming }
        var result = incoming
        if incoming.detailsOmitted == true {
            result.codexMetadata = incoming.codexMetadata ?? codexMetadata
            result.clientDeclared = incoming.clientDeclared ?? clientDeclared
            result.grokMetadata = incoming.grokMetadata ?? grokMetadata
            result.failureDetail = incoming.failureDetail ?? failureDetail
            result.message = incoming.message ?? message
            result.streamTrace = incoming.streamTrace ?? streamTrace
            result.toolCalls = incoming.toolCalls ?? toolCalls
            result.timeoutMS = incoming.timeoutMS ?? timeoutMS
            result.upstreamHost = incoming.upstreamHost ?? upstreamHost
            result.upstreamRequestID = incoming.upstreamRequestID ?? upstreamRequestID
        }
        return result
    }
}
