import Foundation
import SumpterCore

/// 入口表的一行。
struct EndpointDisplayRow: Identifiable, Hashable {
    let id: String
    let name: String
    let baseURL: String
    let protocolName: String
    let enabled: Bool
    let apiKey: String
    let priority: Int
    let stickyGroup: String?
    let keepAlive: Bool
    let mappingCount: Int
    let catalog: ModelCatalog

    init(endpoint: Endpoint) {
        id = endpoint.id
        name = endpoint.name
        baseURL = endpoint.baseURL.absoluteString
        protocolName = endpoint.protocolMode.rawValue
        enabled = endpoint.enabled
        apiKey = endpoint.apiKey
        priority = endpoint.priority
        stickyGroup = endpoint.stickyGroup
        keepAlive = endpoint.keepAlive
        mappingCount = endpoint.mappings.count
        catalog = endpoint.catalog
    }

    var statusText: String { enabled ? "启用" : "停用" }
    var keyStatusText: String { apiKey.isEmpty ? "未配置" : "已配置" }
    var keyTailText: String { apiKey.isEmpty ? "未配置" : "..." + String(apiKey.suffix(4)) }
    var priorityText: String { String(priority) }
    var stickyGroupText: String { stickyGroup ?? "入口 ID（独立组）" }
    var keepAliveText: String { keepAlive ? "启用" : "关闭" }
    var protocolDisplayName: String {
        EndpointProtocolMode(rawValue: protocolName)?.displayName ?? protocolName
    }
    var modelsError: String { catalog.error }
    var modelCatalogText: String {
        catalog.uniqueModels.isEmpty ? "未获取模型" : "已获取 \(catalog.uniqueModels.count) 个"
    }
    var modelCatalogStatusText: String {
        let status = catalog.status.isEmpty ? "未获取" : catalog.status
        let source = catalog.source.isEmpty ? "manual" : catalog.source
        let suffix = catalog.updatedAt.isEmpty ? "" : " · \(catalog.updatedAt)"
        return "模型来源：\(source) · 状态：\(status)\(suffix)"
    }
}

struct MappingDisplayRow: Identifiable, Hashable {
    let id: String
    let endpointID: String
    let endpointName: String
    let clientPattern: String
    let upstreamModel: String
    let thinking: ThinkingMode
    let effort: ReasoningEffort?
    let context: ContextMode
    let failoverTimeoutSeconds: Double?

    init(endpoint: Endpoint, mapping: ModelMapping) {
        id = mapping.id
        endpointID = endpoint.id
        endpointName = endpoint.name
        clientPattern = mapping.clientPattern.rawValue
        upstreamModel = mapping.upstreamModel
        thinking = mapping.thinking
        effort = mapping.effort
        context = mapping.context
        failoverTimeoutSeconds = mapping.failoverTimeoutSeconds
    }

}

struct FeatureRuleDisplayRow: Identifiable, Hashable {
    let id: String
    let name: String
    let enabled: Bool
    let matchSummary: String
    let targetSummary: String
    let isBuiltIn: Bool

    init(rule: FeatureRule, endpoints: [Endpoint]) {
        id = rule.id
        name = rule.name
        enabled = rule.enabled
        isBuiltIn = BuiltInFeatureRules.isBuiltIn(rule.id)
        matchSummary = [
            rule.match.requestKind.map { "严格请求类型: \($0.displayName)" },
            rule.match.toolTypePrefix.map { "Tool: \($0)" },
            rule.match.systemContains.map { "System: \($0)" },
            rule.match.messagesContain.map { "Messages: \($0)" },
            rule.match.modelEquals.map { "Model: \($0)" }
        ].compactMap { $0 }.joined(separator: " / ")
        let effort = rule.target.effortOverride.map { " · effort: \($0.rawValue)" } ?? " · effort: 跟随请求"
        if let endpointID = rule.target.endpointID {
            let endpointName = endpoints.first { $0.id == endpointID }?.name ?? endpointID
            targetSummary = "固定 Provider：\(endpointName) / \(rule.target.model)\(effort)"
        } else {
            targetSummary = "Provider 候选序列 / \(rule.target.model)\(effort)"
        }
    }
}

struct UsageAggregateRow: Identifiable, Hashable {
    let id: String
    let name: String
    let attempts: Int
    let successes: Int
    let failures: Int
    let cancelled: Int
    let failovers: Int
    let averageMS: Int
    let inputTokens: Int
    let outputTokens: Int
    let cacheReadInputTokens: Int
    let inputTokenPresence: Int?
    let outputTokenPresence: Int?
    let cacheReadTokenPresence: Int?
    let processedInputTokens: Int
    let processedTotalTokens: Int
    let cacheCreationInputTokens: Int
    let cacheReadTokenRate: Double?
    let cacheReadRequestRate: Double?
    let cacheCreationTokenRate: Double?
    let cacheCreationTokenPresence: Int?
    let tokenAccountingSemantics: String?

    init(
        id: String,
        name: String,
        attempts: Int,
        successes: Int,
        failures: Int,
        cancelled: Int = 0,
        failovers: Int,
        averageMS: Int,
        inputTokens: Int = 0,
        outputTokens: Int = 0,
        cacheReadInputTokens: Int = 0,
        inputTokenPresence: Int? = 0,
        outputTokenPresence: Int? = 0,
        cacheReadTokenPresence: Int? = 0,
        processedInputTokens: Int = 0,
        processedTotalTokens: Int = 0,
        cacheCreationInputTokens: Int = 0,
        cacheReadTokenRate: Double? = nil,
        cacheReadRequestRate: Double? = nil,
        cacheCreationTokenRate: Double? = nil,
        cacheCreationTokenPresence: Int? = 0,
        tokenAccountingSemantics: String? = nil
    ) {
        self.id = id
        self.name = name
        self.attempts = attempts
        self.successes = successes
        self.failures = failures
        self.cancelled = cancelled
        self.failovers = failovers
        self.averageMS = averageMS
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cacheReadInputTokens = cacheReadInputTokens
        self.inputTokenPresence = inputTokenPresence
        self.outputTokenPresence = outputTokenPresence
        self.cacheReadTokenPresence = cacheReadTokenPresence
        self.processedInputTokens = processedInputTokens
        self.processedTotalTokens = processedTotalTokens
        self.cacheCreationInputTokens = cacheCreationInputTokens
        self.cacheReadTokenRate = cacheReadTokenRate
        self.cacheReadRequestRate = cacheReadRequestRate
        self.cacheCreationTokenRate = cacheCreationTokenRate
        self.cacheCreationTokenPresence = cacheCreationTokenPresence
        self.tokenAccountingSemantics = tokenAccountingSemantics
    }

    var pending: Int {
        max(0, attempts - successes - failures - cancelled)
    }

    /// 排序用的成功率(0…1);没有尝试记为 -1,排序时沉底而不是伪装成 0%。
    var successRate: Double {
        guard attempts > 0 else { return -1 }
        let completed = successes + failures + cancelled
        guard completed > 0 else { return -1 }
        return Double(successes) / Double(completed)
    }

    var successRateText: String {
        guard attempts > 0 else { return "-" }
        guard successRate >= 0 else { return attempts > 0 ? "待定" : "-" }
        let pendingText = pending > 0 ? " · 待定 \(pending)" : ""
        return "\(Int((successRate * 100).rounded()))%\(pendingText)"
    }

    static func endpointRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        // 入口排行只聚合上游尝试;按 endpointID 分组(改名不拆行),展示用 name。
        let completed = events.filter { $0.kind == "upstream" && !$0.isInFlight && !$0.isCancelled }
        let grouped = Dictionary(grouping: completed) { event -> String in
            event.endpointID ?? event.endpointName ?? "未知入口"
        }
        return grouped.map { id, rows in
            let displayName = rows.lazy.compactMap(\.endpointName).first ?? id
            let successes = rows.filter(\.isSucceeded).count
            let failures = rows.filter(\.isFailed).count
            let totalMS = rows.reduce(0) { $0 + $1.durationMS }
            return UsageAggregateRow(
                id: id,
                name: displayName,
                attempts: rows.count,
                successes: successes,
                failures: failures,
                failovers: rows.filter(\.failover).count,
                averageMS: rows.isEmpty ? 0 : totalMS / rows.count
            )
        }
        .sorted(using: Self.stableOrder)
    }

    static func modelRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        // 模型排行只聚合 client:一次请求算一次端到端,避免与 upstream 双重计 / notify「未知模型」。
        aggregate(events: events.filter { $0.kind == "client" }) { event in
            modelChain(event)
        }
    }

    /// 协议路径只聚合 client 事件，代表一次端到端请求。字段不齐时保留明确缺失态，
    /// 不从入口配置、路径 token 或 HTTP 状态反推。
    static func protocolRouteRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        aggregate(events: events.filter { $0.kind == "client" }) { event in
            RuntimeEventPresentation.protocolPath(
                sourceFormat: event.sourceFormat,
                targetFormat: event.targetFormat,
                routeMode: event.routeMode
            )
        }
    }

    /// 未命中特征规则与旧事件没记录是不同状态：空字段表示请求走了普通模型路由。
    static func featureRuleRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        aggregate(events: events.filter { $0.kind == "client" }) { event in
            guard let ruleID = normalized(event.featureRuleID) else { return "未命中特征规则" }
            if let name = BuiltInFeatureRules.canonicalRule(id: ruleID)?.name {
                return "\(name) (\(ruleID))"
            }
            return ruleID
        }
    }

    /// 上游状态只看 upstream 尝试。响应头前失败和旧事件缺字段都没有可报告的 HTTP code，
    /// 因此合并为一个明确的非状态码分组。
    static func upstreamStatusRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        aggregate(events: events.filter { $0.kind == "upstream" }) { event in
            event.upstreamStatusCode.map { "HTTP \($0)" } ?? "响应头前失败 / 未记录"
        }
    }

    static func clientKindRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        // 同上只聚合 client:一次请求一行。旧 stats.json 没有 clientKind,单列出来
        // 避免混进「未知客户端」——那是「UA 认不出」,不是「没记过」。
        aggregate(events: events.filter { $0.kind == "client" }) { event in
            event.clientKind?.displayName ?? "未记录（旧事件或早期拒绝）"
        }
    }

    static func purposeRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        // 用途排行同样只聚合 client；旧 stats.json 没有 requestPurpose，单列出来避免伪装成普通请求。
        aggregate(events: events.filter { $0.kind == "client" }) { event in
            event.requestPurpose?.displayName ?? "旧事件（未记录）"
        }
    }

    /// 失败维度按结构化 failureKind/failurePhase 聚合；没有结构化字段的历史事件单独归档，
    /// 不把 HTTP 200 或一段自由文本误当成失败原因。
    static func failureRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        let failed = events.filter { !$0.isInFlight && !$0.isCancelled && $0.isFailed }
        let grouped = Dictionary(grouping: failed) { event -> String in
            guard let kind = event.failureKind else { return "旧事件（失败原因未记录）" }
            let phase = event.failurePhase.map { " / " + RuntimeEventPresentation.failurePhaseDisplay($0) } ?? ""
            return RuntimeEventPresentation.failureKindDisplay(kind) + phase
        }
        return grouped.map { name, rows in
            UsageAggregateRow(
                id: name,
                name: name,
                attempts: rows.count,
                successes: 0,
                failures: rows.count,
                failovers: rows.filter(\.failover).count,
                averageMS: rows.isEmpty ? 0 : rows.reduce(0) { $0 + $1.durationMS } / rows.count
            )
        }.sorted(using: Self.stableOrder)
    }

    /// 实际工具调用按协议事件中的 toolCalls 展开；请求声明的工具不会被算作已调用。
    static func toolRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        var grouped: [String: [RuntimeEvent]] = [:]
        for event in events where event.kind == "client" && !event.isInFlight && !event.isCancelled {
            for tool in event.toolCalls ?? [] where !tool.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                grouped[tool, default: []].append(event)
            }
        }
        return grouped.map { name, rows in
            let successes = rows.filter(\.isSucceeded).count
            let failures = rows.filter(\.isFailed).count
            return UsageAggregateRow(
                id: name,
                name: name,
                attempts: rows.count,
                successes: successes,
                failures: failures,
                failovers: rows.filter(\.failover).count,
                averageMS: rows.isEmpty ? 0 : rows.reduce(0) { $0 + $1.durationMS } / rows.count
            )
        }.sorted(using: Self.stableOrder)
    }

    /// Codex 元数据按各个独立维度展开。每个维度都带前缀，避免把同名的请求类型、
    /// agent path 或工具 namespace 合并到一行。
    static func codexRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        var grouped: [String: [RuntimeEvent]] = [:]
        func record(_ dimension: String, _ value: String?, event: RuntimeEvent) {
            guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty else { return }
            grouped["\(dimension) · \(value)", default: []].append(event)
        }
        let codexEvents = events.filter {
            $0.kind == "client" && !$0.isInFlight && !$0.isCancelled && $0.codexMetadata != nil
        }
        for event in codexEvents {
            guard let metadata = event.codexMetadata else { continue }
            record("请求类型", metadata.requestKind ?? "未记录", event: event)
            record("代理类型", RuntimeEventDisplay.codexAgentRole(metadata), event: event)
            record("线程来源", metadata.threadSource, event: event)
            record("代理路径", metadata.agentName, event: event)
            for workspace in metadata.workspaces.keys.sorted() {
                record("工作区", workspace, event: event)
            }
            for namespace in metadata.toolNamespacesInfo.keys.sorted() {
                record("工具命名空间", namespace, event: event)
            }
            if let compaction = metadata.compaction {
                let value = [compaction.trigger, compaction.phase, compaction.strategy]
                    .compactMap { $0 }
                    .filter { !$0.isEmpty }
                    .joined(separator: " / ")
                record("Compaction", value.isEmpty ? "已记录" : value, event: event)
            }
        }
        return grouped.map { name, rows in
            let successes = rows.filter(\.isSucceeded).count
            let failures = rows.filter(\.isFailed).count
            return UsageAggregateRow(
                id: name,
                name: name,
                attempts: rows.count,
                successes: successes,
                failures: failures,
                failovers: rows.filter(\.failover).count,
                averageMS: rows.isEmpty ? 0 : rows.reduce(0) { $0 + $1.durationMS } / rows.count
            )
        }.sorted(using: Self.stableOrder)
    }

    /// 流终止事件是协议事实；完成事件没观察到 terminal 时单列，不能按 HTTP 200 补成 completed。
    static func streamTerminalRows(from events: [RuntimeEvent]) -> [UsageAggregateRow] {
        let traced = events.filter { $0.kind == "client" && $0.streamTrace != nil }
        let grouped = Dictionary(grouping: traced) { event -> String in
            let terminal = event.streamTrace?.terminalEvent
                ?? (event.isInFlight ? "等待协议终止" : "未观察到终止")
            return "终止事件 · \(terminal)"
        }
        return grouped.map { name, rows in
            UsageAggregateRow(
                id: name,
                name: name,
                attempts: rows.count,
                successes: rows.filter(\.isSucceeded).count,
                failures: rows.filter(\.isFailed).count,
                failovers: rows.filter(\.failover).count,
                averageMS: rows.isEmpty ? 0 : rows.reduce(0) { $0 + $1.durationMS } / rows.count
            )
        }.sorted(using: Self.stableOrder)
    }

    /// 默认按名称升序 —— 这是唯一在刷新之间恒定的键。
    ///
    /// 曾经默认按 attempts 降序,但排行读的是滑动事件窗口:429 风暴下窗口几秒就换一批,
    /// 每次自动刷新行序都重排,表格看着在疯狂跳动。想看谁最忙点一下「尝试」列头即可。
    static let stableOrder = [KeyPathComparator(\UsageAggregateRow.name, order: .forward)]

    private static func normalized(_ value: String?) -> String? {
        guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty else {
            return nil
        }
        return value
    }

    /// 与最近事件表采用同一模型语义：客户端模型映射到路由定型模型；两者相同只显示一次。
    /// 老事件两者都没有时才回退到实际上游模型。
    private static func modelChain(_ event: RuntimeEvent) -> String {
        let client = displayModel(event.clientModel)
        let effective = displayModel(event.effectiveModel)
        var models: [String] = []
        for model in [client, effective].compactMap({ $0 }) where !models.contains(model) {
            models.append(model)
        }
        if models.isEmpty, let upstream = displayModel(event.upstreamModel) {
            models.append(upstream)
        }
        return models.isEmpty ? "未知模型" : models.joined(separator: " → ")
    }

    private static func displayModel(_ value: String?) -> String? {
        guard let value = normalized(value) else { return nil }
        let cleaned = ModelName.clean(value)
        guard !cleaned.isEmpty else { return nil }
        return cleaned
    }

    private static func aggregate(events: [RuntimeEvent], key: (RuntimeEvent) -> String) -> [UsageAggregateRow] {
        // 进行中的事件没有最终状态/耗时,不进排行(否则拉低平均延迟、误算成败)。
        let grouped = Dictionary(grouping: events.filter { !$0.isInFlight && !$0.isCancelled }, by: key)
        return grouped.map { name, rows in
            let successes = rows.filter(\.isSucceeded).count
            let failures = rows.filter(\.isFailed).count
            let totalMS = rows.reduce(0) { $0 + $1.durationMS }
            return UsageAggregateRow(
                id: name,
                name: name,
                attempts: rows.count,
                successes: successes,
                failures: failures,
                failovers: rows.filter(\.failover).count,
                averageMS: rows.isEmpty ? 0 : totalMS / rows.count
            )
        }
        .sorted(using: Self.stableOrder)
    }
}

/// 统计页首屏使用的诊断摘要。它只计算事实字段，明示“未记录”和“未观察到”的边界。
struct RuntimeDiagnosticSummary: Equatable {
    let failedEvents: Int
    let structuredFailureEvents: Int
    let explicitUnknownClients: Int
    let unrecordedClientKinds: Int
    let streamedEvents: Int
    let observedChunks: Int
    let maxChunkGapMS: Int?
    let missingTerminalEvents: Int
    let toolCallEvents: Int
    let toolCallCount: Int
    let codexEvents: Int
    let inFlightEvents: Int
    let outcomeRecordedEvents: Int
    let completedWithoutPhaseEvents: Int

    init(events: [RuntimeEvent]) {
        let completed = events.filter { !$0.isInFlight && !$0.isCancelled }
        failedEvents = completed.filter(\.isFailed).count
        structuredFailureEvents = completed.filter { $0.isFailed && $0.failureKind != nil }.count
        explicitUnknownClients = events.filter { $0.kind == "client" && $0.clientKind == .unknown }.count
        unrecordedClientKinds = events.filter { $0.kind == "client" && $0.clientKind == nil }.count
        let clientEvents = events.filter { $0.kind == "client" }
        let traced = clientEvents.compactMap(\.streamTrace)
        streamedEvents = traced.count
        observedChunks = traced.compactMap(\.chunkCount).reduce(0, +)
        maxChunkGapMS = traced.compactMap(\.maxChunkGapMS).max()
        missingTerminalEvents = traced.filter { $0.terminalEvent == nil }.count
        let toolEvents = clientEvents.filter { !($0.toolCalls ?? []).isEmpty }
        toolCallEvents = toolEvents.count
        toolCallCount = toolEvents.reduce(0) { $0 + ($1.toolCalls?.count ?? 0) }
        codexEvents = clientEvents.filter { $0.codexMetadata != nil }.count
        inFlightEvents = events.filter(\.isInFlight).count
        outcomeRecordedEvents = events.filter { $0.outcome != nil }.count
        completedWithoutPhaseEvents = events.filter { !$0.isInFlight && $0.phase == nil && $0.outcome != nil }.count
    }
}

/// 最近事件窗口的主运行指标；只以一条 client 事件代表一次端到端请求。
struct RuntimeOperationalSummary: Equatable {
    let requests: Int
    let successes: Int
    let failures: Int
    let cancellations: Int
    let failovers: Int
    let averageTTFBMS: Int?
    let averageDurationMS: Int?

    init(events: [RuntimeEvent]) {
        let clientEvents = events.filter { $0.kind == "client" && !$0.isInFlight }
        requests = clientEvents.count
        successes = clientEvents.filter(\.isSucceeded).count
        failures = clientEvents.filter(\.isFailed).count
        cancellations = clientEvents.filter(\.isCancelled).count
        failovers = clientEvents.filter(\.failover).count
        let ttfbValues = clientEvents.compactMap(\.ttfbMS)
        averageTTFBMS = ttfbValues.isEmpty ? nil : ttfbValues.reduce(0, +) / ttfbValues.count
        averageDurationMS = clientEvents.isEmpty
            ? nil
            : clientEvents.reduce(0) { $0 + $1.durationMS } / clientEvents.count
    }

    var successRateText: String {
        guard requests > 0 else { return "-" }
        return "\(Int((Double(successes) / Double(requests) * 100).rounded()))%"
    }
}

extension AppConfig {
    var endpointRows: [EndpointDisplayRow] {
        endpoints.map { EndpointDisplayRow(endpoint: $0) }
    }

    func endpointRows(providerID: String? = nil) -> [EndpointDisplayRow] {
        let filtered = providerID.map { id in endpoints.filter { $0.id == id } } ?? endpoints
        return filtered.map { EndpointDisplayRow(endpoint: $0) }
    }

    func modelSummary() -> String {
        let models = endpoints
            .flatMap { $0.mappings.map(\.clientPattern.rawValue) }
            .reduce(into: (seen: Set<String>(), values: [String]())) { result, raw in
                let cleaned = ModelName.clean(raw)
                guard !cleaned.isEmpty, result.seen.insert(cleaned).inserted else { return }
                result.values.append(raw)
            }
            .values
        if !models.isEmpty { return models.joined(separator: ", ") }
        return "暂无入口模型映射"
    }
}
