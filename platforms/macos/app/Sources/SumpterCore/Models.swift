import Foundation

public enum ProviderProtocol: String, Codable, Sendable, CaseIterable {
    case anthropic
    case openai
    /// OpenAI Responses API(/v1/responses)——2025 起的官方主力端点,codex 系列仅此可用。
    /// `openai` 保留为 chat/completions(vLLM/Ollama 等兼容生态的事实标准)。
    case openaiResponses = "openai-responses"
}

/// 入口在配置文件中声明的协议能力模式。`auto` 不是可发送给上游的真实协议，
/// 它会在每次路由时解析为当前请求的 SourceFormat（或分流规则显式指定的目标协议）。
public enum EndpointProtocolMode: String, Codable, Sendable, CaseIterable {
    case auto
    case anthropic
    case openai
    case openaiResponses = "openai-responses"

    public var fixedProtocol: ProviderProtocol? {
        switch self {
        case .auto:
            nil
        case .anthropic:
            .anthropic
        case .openai:
            .openai
        case .openaiResponses:
            .openaiResponses
        }
    }

    /// 把入口配置模式解析成一次请求实际使用的 TargetFormat。
    /// 分流规则指定目标协议时，固定协议入口必须与其完全一致；Auto 可解析为任意真实协议。
    public func resolve(sourceFormat: ProviderProtocol, targetOverride: ProviderProtocol? = nil) -> ProviderProtocol? {
        if let targetOverride {
            return self == .auto || fixedProtocol == targetOverride ? targetOverride : nil
        }
        return fixedProtocol ?? sourceFormat
    }

    public var displayName: String {
        switch self {
        case .auto:
            "自动（三协议）"
        case .anthropic:
            "Anthropic"
        case .openai:
            "OpenAI Chat"
        case .openaiResponses:
            "OpenAI Responses"
        }
    }
}

/// 仅用于读取 schema v3-v5 的 legacy migration；新配置不编码池角色。
@available(*, deprecated, message: "Provider 池已由扁平 endpoints 取代，仅兼容迁移读取")
public enum PoolRole: String, Codable, Sendable, CaseIterable {
    case primary
    case fallback
    case custom
}

public enum ThinkingMode: String, Codable, Sendable, CaseIterable {
    case disabled
    case passthrough
    case adaptive
}

/// CPA / CLIProxyAPI 兼容的 reasoning effort 档位(模型名后缀 `model(high)`)。
/// 与 CPA `ParseLevelSuffix` / 特殊值一致;`ultra` 故意不在列表中。
public enum ReasoningEffort: String, Codable, Sendable, CaseIterable {
    case none
    case auto
    case minimal
    case low
    case medium
    case high
    case xhigh
    case max

    public var displayName: String {
        switch self {
        case .none: return "关闭推理"
        case .auto: return "自动"
        case .minimal: return "最低"
        case .low: return "低"
        case .medium: return "中"
        case .high: return "高"
        case .xhigh: return "很高"
        case .max: return "最高"
        }
    }
}

public enum ContextMode: String, Codable, Sendable, CaseIterable {
    case standard
    case oneMillion
    case strip

    public var displayName: String {
        switch self {
        case .standard:
            "透传"
        case .oneMillion:
            "1M 上下文"
        case .strip:
            "剥离 1M"
        }
    }
}

public let anthropicContext1MBeta = "context-1m-2025-08-07"

/// 客户端模型匹配规则:精确名或 `claude-opus-*` 这种前缀通配。配置里就是一个裸字符串。
public struct ModelPattern: Codable, Equatable, Hashable, Sendable, ExpressibleByStringLiteral {
    public var rawValue: String

    public init(_ rawValue: String) {
        self.rawValue = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    public init(stringLiteral value: StringLiteralType) {
        self.init(value)
    }

    public init(from decoder: Decoder) throws {
        self.init(try decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }

    public func matches(_ model: String) -> Bool {
        let cleanedModel = ModelName.clean(model)
        let pattern = ModelName.clean(rawValue)
        guard !pattern.isEmpty else {
            return false
        }
        if pattern.hasSuffix("*") {
            return cleanedModel.hasPrefix(String(pattern.dropLast()))
        }
        return cleanedModel == pattern
    }
}

public struct ListenerConfig: Codable, Equatable, Sendable {
    public var host: String
    public var port: Int
    public var allowedCIDRs: [String]
    /// 入站 Auth Token;空 = 不校验(仅环回可达时的默认状态)。
    public var authToken: String

    enum CodingKeys: String, CodingKey {
        case host
        case port
        case allowedCIDRs
        case authToken
    }

    public init(
        host: String = "127.0.0.1",
        port: Int = 57_878,
        allowedCIDRs: [String] = [],
        authToken: String = ""
    ) {
        self.host = host
        self.port = port
        self.allowedCIDRs = allowedCIDRs
        self.authToken = authToken
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        host = try keyed.decodeIfPresent(String.self, forKey: .host) ?? "127.0.0.1"
        port = try keyed.decodeIfPresent(Int.self, forKey: .port) ?? 57_878
        allowedCIDRs = try keyed.decodeIfPresent([String].self, forKey: .allowedCIDRs) ?? []
        authToken = try keyed.decodeIfPresent(String.self, forKey: .authToken) ?? ""
    }

    public var hasInboundAuth: Bool {
        !authToken.isEmpty
    }
}

/// 转发与重试参数,全局单份(旧版每个池各存一份但必须一致)。
public struct RetryPolicy: Codable, Equatable, Sendable {
    /// 首响应总截止秒数;nil = 由客户端决定,不设截止。
    public var responseTimeoutSeconds: Double?
    /// 流式响应两次吐字之间的最长间隔;nil = 允许无限空闲。
    public var streamIdleTimeoutSeconds: Double?
    /// 单个入口收到 HTTP 500 后的额外重试次数;0 = 不在该入口重试。
    public var max500Retries: Int
    /// 当前入口 HTTP 500 重试耗尽后是否切换到下一个入口。
    public var failoverOn500: Bool
    /// 最终失败响应中可透传的 retry_delay 秒数;nil = 未配置。
    public var retryDelaySeconds: Double?
    /// 是否把 retry_delay 与 Retry-After 透传给客户端。
    public var passThroughRetryDelay: Bool
    /// 所有可重试状态与首响应前网络故障的最大轮数;0 = 不限轮数。
    public var maxDeferredRounds: Int
    /// 所有可重试状态与首响应前网络故障的跨轮总上限秒数;0 = 不限。
    public var maxRetryDurationSeconds: Double
    /// 同一次请求中当前粘性调度组在非 500 可重试故障后的额外尝试次数;0 = 首次失败后立即切换。
    public var sessionStickyRetries: Int
    /// 下面两组状态码不可配,不进配置文件。
    public var retryableStatusCodes: Set<Int>
    public var deferredStatusCodes: Set<Int>

    enum CodingKeys: String, CodingKey {
        case responseTimeoutSeconds
        case streamIdleTimeoutSeconds
        case max500Retries
        case failoverOn500
        case retryDelaySeconds
        case passThroughRetryDelay
        case maxDeferredRounds
        case maxRetryDurationSeconds
        case sessionStickyRetries
    }

    public init(
        responseTimeoutSeconds: Double? = nil,
        streamIdleTimeoutSeconds: Double? = nil,
        max500Retries: Int = 0,
        failoverOn500: Bool = true,
        retryDelaySeconds: Double? = nil,
        passThroughRetryDelay: Bool = true,
        maxDeferredRounds: Int = 0,
        maxRetryDurationSeconds: Double = 0,
        sessionStickyRetries: Int = 2,
        retryableStatusCodes: Set<Int> = [401, 402, 403, 429, 502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 529, 530],
        deferredStatusCodes: Set<Int> = [401, 402, 403, 429, 502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 529, 530]
    ) {
        self.responseTimeoutSeconds = responseTimeoutSeconds
        self.streamIdleTimeoutSeconds = streamIdleTimeoutSeconds
        self.max500Retries = max(0, max500Retries)
        self.failoverOn500 = failoverOn500
        self.retryDelaySeconds = retryDelaySeconds
        self.passThroughRetryDelay = passThroughRetryDelay
        self.maxDeferredRounds = max(0, maxDeferredRounds)
        self.maxRetryDurationSeconds = max(0, maxRetryDurationSeconds)
        self.sessionStickyRetries = max(0, sessionStickyRetries)
        self.retryableStatusCodes = retryableStatusCodes
        self.deferredStatusCodes = deferredStatusCodes
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            responseTimeoutSeconds: try keyed.decodeIfPresent(Double.self, forKey: .responseTimeoutSeconds),
            streamIdleTimeoutSeconds: try keyed.decodeIfPresent(Double.self, forKey: .streamIdleTimeoutSeconds),
            max500Retries: try keyed.decodeIfPresent(Int.self, forKey: .max500Retries) ?? 0,
            failoverOn500: try keyed.decodeIfPresent(Bool.self, forKey: .failoverOn500) ?? true,
            retryDelaySeconds: try keyed.decodeIfPresent(Double.self, forKey: .retryDelaySeconds),
            passThroughRetryDelay: try keyed.decodeIfPresent(Bool.self, forKey: .passThroughRetryDelay) ?? true,
            maxDeferredRounds: try keyed.decodeIfPresent(Int.self, forKey: .maxDeferredRounds) ?? 0,
            maxRetryDurationSeconds: try keyed.decodeIfPresent(Double.self, forKey: .maxRetryDurationSeconds) ?? 0,
            sessionStickyRetries: try keyed.decodeIfPresent(Int.self, forKey: .sessionStickyRetries) ?? 2
        )
    }

    public func encode(to encoder: Encoder) throws {
        var keyed = encoder.container(keyedBy: CodingKeys.self)
        // 显式写 null 而不是省略:让「不设截止」在配置文件里是可见的选择。
        try keyed.encode(responseTimeoutSeconds, forKey: .responseTimeoutSeconds)
        try keyed.encode(streamIdleTimeoutSeconds, forKey: .streamIdleTimeoutSeconds)
        try keyed.encode(max500Retries, forKey: .max500Retries)
        try keyed.encode(failoverOn500, forKey: .failoverOn500)
        try keyed.encode(retryDelaySeconds, forKey: .retryDelaySeconds)
        try keyed.encode(passThroughRetryDelay, forKey: .passThroughRetryDelay)
        try keyed.encode(maxDeferredRounds, forKey: .maxDeferredRounds)
        try keyed.encode(maxRetryDurationSeconds, forKey: .maxRetryDurationSeconds)
        try keyed.encode(sessionStickyRetries, forKey: .sessionStickyRetries)
    }
}

/// 「获取模型」拉回来的目录及其状态,纯展示用,不参与路由。
public struct ModelCatalog: Codable, Equatable, Hashable, Sendable {
    public var models: [String]
    public var source: String
    public var status: String
    public var error: String
    public var updatedAt: String

    enum CodingKeys: String, CodingKey {
        case models
        case source
        case status
        case error
        case updatedAt
    }

    public init(
        models: [String] = [],
        source: String = "",
        status: String = "",
        error: String = "",
        updatedAt: String = ""
    ) {
        self.models = Self.deduplicatedModels(models)
        self.source = source
        self.status = status
        self.error = error
        self.updatedAt = updatedAt
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        models = Self.deduplicatedModels(
            try keyed.decodeIfPresent([String].self, forKey: .models) ?? []
        )
        source = try keyed.decodeIfPresent(String.self, forKey: .source) ?? ""
        status = try keyed.decodeIfPresent(String.self, forKey: .status) ?? ""
        error = try keyed.decodeIfPresent(String.self, forKey: .error) ?? ""
        updatedAt = try keyed.decodeIfPresent(String.self, forKey: .updatedAt) ?? ""
    }

    /// 供应商目录偶尔会重复返回同一个模型，或在模型名两侧带空格。
    /// 保留首次出现的顺序，避免表格计数和「从已知模型添加」出现重复项。
    public static func deduplicatedModels(_ models: [String]) -> [String] {
        var seen = Set<String>()
        return models.compactMap { raw in
            let model = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            let key = ModelName.clean(model)
            guard !key.isEmpty, seen.insert(key).inserted else { return nil }
            return model
        }
    }

    /// 已去空白且稳定去重后的展示目录。该属性也覆盖旧配置中尚未重新保存的重复值。
    public var uniqueModels: [String] {
        Self.deduplicatedModels(models)
    }

    public var isEmpty: Bool {
        models.isEmpty && source.isEmpty && status.isEmpty && error.isEmpty && updatedAt.isEmpty
    }
}

/// 入口自带的模型映射:客户端模型 → 上游模型,附带 thinking / 1M / 首个超时。
public struct ModelMapping: Codable, Equatable, Sendable, Identifiable {
    /// 列表选中用的稳定标识:**从内容派生**(清洗后的 clientPattern),不进配置文件。
    ///
    /// 旧版每次 decode 现生成 UUID:一旦「重新加载配置」或外部 `/__reload` 触发重读,
    /// 全部映射 id 换新 —— 表格选中丢失,若此时开着编辑 sheet,保存会报「模型映射不存在」。
    /// sidecar 架构下外部 reload 更频繁(SSE config_reloaded),必须稳定。
    /// 同一入口内 clientPattern 唯一(保存时校验),故足以标识。
    public var id: String { ModelName.clean(clientPattern.rawValue) }
    public var clientPattern: ModelPattern
    public var upstreamModel: String
    public var thinking: ThinkingMode
    public var context: ContextMode
    /// 该映射的首个响应超时;nil = 不额外设截止。
    public var failoverTimeoutSeconds: Double?
    /// Mapping-level capabilities. Empty keeps the Rust-side name inference.
    /// This is persisted for forward compatibility; the current editor does
    /// not expose a capability picker yet.
    public var capabilities: [String]

    enum CodingKeys: String, CodingKey {
        case clientPattern
        case upstreamModel
        case thinking
        case context
        case failoverTimeoutSeconds
        case capabilities
    }

    public init(
        clientPattern: ModelPattern,
        upstreamModel: String = "",
        thinking: ThinkingMode = .disabled,
        context: ContextMode = .standard,
        failoverTimeoutSeconds: Double? = nil,
        capabilities: [String] = []
    ) {
        self.clientPattern = clientPattern
        self.upstreamModel = upstreamModel
        self.thinking = thinking
        self.context = context
        self.failoverTimeoutSeconds = failoverTimeoutSeconds
        self.capabilities = capabilities
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        clientPattern = try keyed.decode(ModelPattern.self, forKey: .clientPattern)
        upstreamModel = ModelName.clean(try keyed.decodeIfPresent(String.self, forKey: .upstreamModel) ?? "")
        thinking = try keyed.decodeIfPresent(ThinkingMode.self, forKey: .thinking) ?? .disabled
        context = try keyed.decodeIfPresent(ContextMode.self, forKey: .context) ?? .standard
        failoverTimeoutSeconds = try keyed.decodeIfPresent(Double.self, forKey: .failoverTimeoutSeconds)
        capabilities = try keyed.decodeIfPresent([String].self, forKey: .capabilities) ?? []
    }

    public func encode(to encoder: Encoder) throws {
        var keyed = encoder.container(keyedBy: CodingKeys.self)
        try keyed.encode(clientPattern, forKey: .clientPattern)
        try keyed.encode(upstreamModel, forKey: .upstreamModel)
        try keyed.encode(thinking, forKey: .thinking)
        try keyed.encode(context, forKey: .context)
        try keyed.encodeIfPresent(failoverTimeoutSeconds, forKey: .failoverTimeoutSeconds)
        if !capabilities.isEmpty {
            try keyed.encode(capabilities, forKey: .capabilities)
        }
    }

    /// 上游模型名:留空表示与客户端模型同名。
    public func upstreamModel(for clientModel: String) -> String {
        upstreamModel.isEmpty ? clientModel : upstreamModel
    }
}

/// 一个真实的上游入口:自带地址、Key、协议与模型映射。
public struct Endpoint: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var baseURL: URL
    public var protocolMode: EndpointProtocolMode
    public var enabled: Bool
    /// 明文 API Key(配置文件权限 0600);空 = 未配置。
    public var apiKey: String
    /// Provider 调度优先级：数值越小越优先；同级保持配置顺序。
    public var priority: Int
    /// 粘性分组:同组入口共享会话粘性。nil = 使用入口 id 作为独立组。
    public var stickyGroup: String?
    public var catalog: ModelCatalog
    /// 自带映射:这个入口**只**承接映射里声明的客户端模型。
    public var mappings: [ModelMapping]
    /// 【实验】出站连接复用(sumpterd 侧生效);新入口编辑器默认 true。
    /// 入口编辑器可直接设置；旧配置缺省仍解码为 false，保存时保留省略策略。
    public var keepAlive: Bool
    enum CodingKeys: String, CodingKey {
        case id
        case name
        case baseURL
        case protocolMode = "protocol"
        case enabled
        case apiKey
        case priority
        case stickyGroup
        case catalog
        case mappings
        case keepAlive
    }

    public init(
        id: String,
        name: String,
        baseURL: URL,
        protocolMode: EndpointProtocolMode = .auto,
        enabled: Bool = true,
        apiKey: String = "",
        priority: Int = 0,
        stickyGroup: String? = nil,
        catalog: ModelCatalog = ModelCatalog(),
        mappings: [ModelMapping] = [],
        keepAlive: Bool = true
    ) {
        self.id = id
        self.name = name
        self.baseURL = baseURL
        self.protocolMode = protocolMode
        self.enabled = enabled
        self.apiKey = apiKey
        self.priority = max(0, priority)
        self.stickyGroup = Endpoint.normalizedGroup(stickyGroup)
        self.catalog = catalog
        self.mappings = mappings
        self.keepAlive = keepAlive
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        id = try keyed.decode(String.self, forKey: .id)
        name = try keyed.decodeIfPresent(String.self, forKey: .name) ?? id
        baseURL = try keyed.decode(URL.self, forKey: .baseURL)
        protocolMode = try keyed.decodeIfPresent(EndpointProtocolMode.self, forKey: .protocolMode) ?? .auto
        enabled = try keyed.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        apiKey = try keyed.decodeIfPresent(String.self, forKey: .apiKey) ?? ""
        priority = max(0, try keyed.decodeIfPresent(Int.self, forKey: .priority) ?? 0)
        stickyGroup = Endpoint.normalizedGroup(try keyed.decodeIfPresent(String.self, forKey: .stickyGroup))
        catalog = try keyed.decodeIfPresent(ModelCatalog.self, forKey: .catalog) ?? ModelCatalog()
        mappings = try keyed.decodeIfPresent([ModelMapping].self, forKey: .mappings) ?? []
        keepAlive = try keyed.decodeIfPresent(Bool.self, forKey: .keepAlive) ?? false
    }

    public func encode(to encoder: Encoder) throws {
        var keyed = encoder.container(keyedBy: CodingKeys.self)
        try keyed.encode(id, forKey: .id)
        try keyed.encode(name, forKey: .name)
        try keyed.encode(baseURL, forKey: .baseURL)
        try keyed.encode(protocolMode, forKey: .protocolMode)
        try keyed.encode(enabled, forKey: .enabled)
        try keyed.encode(apiKey, forKey: .apiKey)
        if priority > 0 {
            try keyed.encode(priority, forKey: .priority)
        }
        try keyed.encodeIfPresent(stickyGroup, forKey: .stickyGroup)
        if !catalog.isEmpty {
            try keyed.encode(catalog, forKey: .catalog)
        }
        try keyed.encode(mappings, forKey: .mappings)
        // 与 sumpterd 的省略策略一致:false 不落盘。
        if keepAlive {
            try keyed.encode(keepAlive, forKey: .keepAlive)
        }
    }

    public func mapping(for clientModel: String, exactOnly: Bool = false) -> ModelMapping? {
        let cleaned = ModelName.clean(clientModel)
        return mappings.first { mapping in
            if exactOnly {
                return ModelName.clean(mapping.clientPattern.rawValue) == cleaned
            }
            return mapping.clientPattern.matches(clientModel)
        }
    }

    /// 精确模型映射优先于通配映射；这样旧池级规则迁移出的 `foo-*` 不会
    /// 遮蔽用户后来添加的 `foo-special` 精确映射。
    public func preferredMapping(for clientModel: String) -> ModelMapping? {
        mapping(for: clientModel, exactOnly: true) ?? mapping(for: clientModel)
    }

    public func hasMapping(clientPattern: String, excluding mappingID: String? = nil) -> Bool {
        let cleaned = ModelName.clean(clientPattern)
        guard !cleaned.isEmpty else {
            return false
        }
        return mappings.contains { mapping in
            if let excludingMappingID = mappingID, mapping.id == excludingMappingID {
                return false
            }
            return ModelName.clean(mapping.clientPattern.rawValue) == cleaned
        }
    }

    private static func normalizedGroup(_ raw: String?) -> String? {
        guard let trimmed = raw?.trimmingCharacters(in: .whitespacesAndNewlines), !trimmed.isEmpty else {
            return nil
        }
        return trimmed
    }
}

/// 旧池形状的兼容读取模型。AppConfig 的正式 wire 永远只写 endpoints。
@available(*, deprecated, message: "Provider 池已由扁平 endpoints 取代，仅兼容迁移读取")
public struct Pool: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var role: PoolRole
    /// Provider 按会话粘性分流、同组线路按配置顺序故障转移。
    public var endpoints: [Endpoint]

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case role
        case endpoints
    }

    public init(
        id: String,
        name: String,
        role: PoolRole,
        endpoints: [Endpoint] = []
    ) {
        self.id = id
        self.name = name
        self.role = role
        self.endpoints = endpoints
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        id = try keyed.decode(String.self, forKey: .id)
        name = try keyed.decodeIfPresent(String.self, forKey: .name) ?? id
        role = try keyed.decodeIfPresent(PoolRole.self, forKey: .role) ?? .custom
        endpoints = try keyed.decodeIfPresent([Endpoint].self, forKey: .endpoints) ?? []
    }

    /// 这个池承接哪些客户端模型 = 各入口自带映射。
    /// 派生值,不进配置文件 —— 不存在「忘了同步」的可能。
    public var acceptedModels: [String] {
        var seen: Set<String> = []
        var output: [String] = []
        for raw in endpoints.flatMap({ $0.mappings.map(\.clientPattern.rawValue) }) {
            let cleaned = ModelName.clean(raw)
            guard !cleaned.isEmpty, !seen.contains(cleaned) else {
                continue
            }
            seen.insert(cleaned)
            output.append(raw)
        }
        return output
    }

    public func matches(model: String) -> Bool {
        endpoints.contains { endpoint in
            endpoint.mappings.contains { $0.clientPattern.matches(model) }
        }
    }

    public func endpoint(id endpointID: String) -> Endpoint? {
        endpoints.first { $0.id == endpointID }
    }
}

/// 内建特征路由所识别的 Claude Code 请求类型。
///
/// 详细 wire 指纹集中在 `RequestInspector` 维护；配置只存稳定的语义名，
/// 避免把「任意历史消息含某子串」这类宽泛条件当成内建规则。
public enum FeatureRequestKind: String, Codable, Equatable, Sendable, CaseIterable {
    case websearch
    case webfetch
    case classifier

    public var displayName: String {
        switch self {
        case .websearch: "WebSearch"
        case .webfetch: "WebFetch"
        case .classifier: "自动模式分类器"
        }
    }
}

public struct FeatureMatch: Codable, Equatable, Sendable {
    /// 保留给内建规则的严格请求类型；其他字段仍是自定义规则的通用 AND 条件。
    public var requestKind: FeatureRequestKind?
    public var toolTypePrefix: String?
    public var systemContains: String?
    public var messagesContain: String?
    public var modelEquals: String?

    public init(
        requestKind: FeatureRequestKind? = nil,
        toolTypePrefix: String? = nil,
        systemContains: String? = nil,
        messagesContain: String? = nil,
        modelEquals: String? = nil
    ) {
        self.requestKind = requestKind
        self.toolTypePrefix = toolTypePrefix
        self.systemContains = systemContains
        self.messagesContain = messagesContain
        self.modelEquals = modelEquals
    }
}

public struct RouteTarget: Codable, Equatable, Sendable {
    /// 非 nil = 命中后只走这个 Provider；nil = 按候选序列故障转移。
    public var endpointID: String?
    /// nil = 跟随客户端原请求；非 nil = 命中分流规则后强制覆盖。
    public var effortOverride: ReasoningEffort?
    public var model: String
    /// nil = 继承入口自身协议。
    public var protocolOverride: ProviderProtocol?

    enum CodingKeys: String, CodingKey {
        case endpointID
        case effortOverride = "effort"
        case model
        case protocolOverride = "protocol"
    }

    public init(
        model: String,
        protocolOverride: ProviderProtocol? = nil,
        endpointID: String? = nil,
        effortOverride: ReasoningEffort? = nil
    ) {
        self.model = ModelName.clean(model)
        self.protocolOverride = protocolOverride
        self.effortOverride = effortOverride
        self.endpointID = endpointID.flatMap { id in
            let trimmed = id.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }
    }

    /// Legacy source compatibility. The pool selector is deliberately ignored
    /// and never encoded; endpointID is the only fixed route target.
    @available(*, deprecated, message: "使用 RouteTarget(model: endpointID:)")
    public init(
        poolID _: String,
        model: String,
        protocolOverride: ProviderProtocol? = nil,
        endpointID: String? = nil,
        effortOverride: ReasoningEffort? = nil
    ) {
        self.init(model: model, protocolOverride: protocolOverride, endpointID: endpointID, effortOverride: effortOverride)
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            model: ModelName.clean(try keyed.decodeIfPresent(String.self, forKey: .model) ?? ""),
            protocolOverride: try keyed.decodeIfPresent(ProviderProtocol.self, forKey: .protocolOverride),
            endpointID: try keyed.decodeIfPresent(String.self, forKey: .endpointID),
            effortOverride: try keyed.decodeIfPresent(ReasoningEffort.self, forKey: .effortOverride)
        )
    }

    /// Legacy UI/source shim only. It is not a stored property and is never
    /// present in Codable output, so the pool concept cannot leak back to the
    /// v6 configuration contract.
    @available(*, deprecated, message: "Provider 池已移除；请使用 endpointID")
    public var poolID: String {
        get { "providers" }
        set { /* ignored for legacy callers */ }
    }

    public func encode(to encoder: Encoder) throws {
        var keyed = encoder.container(keyedBy: CodingKeys.self)
        try keyed.encodeIfPresent(endpointID, forKey: .endpointID)
        try keyed.encodeIfPresent(effortOverride, forKey: .effortOverride)
        try keyed.encode(model, forKey: .model)
        try keyed.encodeIfPresent(protocolOverride, forKey: .protocolOverride)
    }
}

public struct FeatureRule: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var enabled: Bool
    public var match: FeatureMatch
    public var target: RouteTarget

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case enabled
        case match
        case target
    }

    public init(
        id: String,
        name: String,
        enabled: Bool = false,
        match: FeatureMatch,
        target: RouteTarget
    ) {
        self.id = id
        self.name = name
        self.enabled = enabled
        self.match = match
        self.target = target
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        id = try keyed.decode(String.self, forKey: .id)
        name = try keyed.decodeIfPresent(String.self, forKey: .name) ?? id
        enabled = try keyed.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
        match = try keyed.decodeIfPresent(FeatureMatch.self, forKey: .match) ?? FeatureMatch()
        target = try keyed.decode(RouteTarget.self, forKey: .target)
    }
}

public enum BuiltInFeatureRules {
    public static let rules: [FeatureRule] = [
        FeatureRule(
            id: "websearch",
            name: "WebSearch",
            match: FeatureMatch(requestKind: .websearch),
            target: RouteTarget(model: "claude-haiku-4-5-20251001")
        ),
        FeatureRule(
            id: "webfetch",
            name: "WebFetch",
            match: FeatureMatch(requestKind: .webfetch),
            target: RouteTarget(model: "claude-haiku-4-5-20251001")
        ),
        FeatureRule(
            id: "classifier",
            name: "安全分类器",
            match: FeatureMatch(requestKind: .classifier),
            target: RouteTarget(model: "claude-haiku-4-5-20251001")
        )
    ]

    public static let ids = Set(rules.map(\.id))

    public static func isBuiltIn(_ id: String) -> Bool {
        ids.contains(id)
    }

    public static func canonicalRule(id: String) -> FeatureRule? {
        rules.first { $0.id == id }
    }

    /// 内建规则的名称与匹配条件由本类型维护:配置里被改坏也会被纠回来,
    /// 只保留用户可编辑的 enabled 与 target。自定义规则原样保留。
    public static func normalized(_ input: [FeatureRule]) -> [FeatureRule] {
        let customRules = input.filter { !isBuiltIn($0.id) }
        let normalizedBuiltIns = rules.map { canonical in
            guard let existing = input.first(where: { $0.id == canonical.id }) else {
                return canonical
            }
            return FeatureRule(
                id: canonical.id,
                name: canonical.name,
                enabled: existing.enabled,
                match: canonical.match,
                target: existing.target
            )
        }
        return normalizedBuiltIns + customRules
    }
}

public struct AppConfig: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = 6

    public var schemaVersion: Int
    public var listener: ListenerConfig
    /// 转发与重试参数,全局共享。
    public var retry: RetryPolicy
    /// 正式配置模型：Provider 候选按用户配置顺序扁平保存。
    public var endpoints: [Endpoint]
    public var featureRules: [FeatureRule]

    enum CodingKeys: String, CodingKey {
        case schemaVersion
        case listener
        case retry
        case endpoints
        case featureRules
    }

    public init(
        schemaVersion: Int = AppConfig.currentSchemaVersion,
        listener: ListenerConfig = ListenerConfig(),
        retry: RetryPolicy = RetryPolicy(),
        endpoints: [Endpoint] = [],
        featureRules: [FeatureRule] = []
    ) {
        self.schemaVersion = schemaVersion
        self.listener = listener
        self.retry = retry
        self.endpoints = endpoints
        self.featureRules = featureRules
    }

    /// Legacy initializer retained for source compatibility with older App
    /// extensions and tests. It flattens pools in their existing order and is
    /// never reflected in the encoded v6 document.
    @available(*, deprecated, message: "Provider 池已移除；请使用 endpoints")
    public init(
        schemaVersion: Int = AppConfig.currentSchemaVersion,
        listener: ListenerConfig = ListenerConfig(),
        retry: RetryPolicy = RetryPolicy(),
        pools: [Pool],
        featureRules: [FeatureRule] = []
    ) {
        self.init(schemaVersion: schemaVersion, listener: listener, retry: retry,
                  endpoints: pools.flatMap(\.endpoints), featureRules: featureRules)
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try keyed.decodeIfPresent(Int.self, forKey: .schemaVersion) ?? AppConfig.currentSchemaVersion
        listener = try keyed.decodeIfPresent(ListenerConfig.self, forKey: .listener) ?? ListenerConfig()
        retry = try keyed.decodeIfPresent(RetryPolicy.self, forKey: .retry) ?? RetryPolicy()
        endpoints = try keyed.decodeIfPresent([Endpoint].self, forKey: .endpoints) ?? []
        featureRules = BuiltInFeatureRules.normalized(
            try keyed.decodeIfPresent([FeatureRule].self, forKey: .featureRules) ?? []
        )
    }

    public func encode(to encoder: Encoder) throws {
        var keyed = encoder.container(keyedBy: CodingKeys.self)
        // New writes are always the current flat schema. ConfigStore performs
        // the version/validation gate before this method is called.
        try keyed.encode(AppConfig.currentSchemaVersion, forKey: .schemaVersion)
        try keyed.encode(listener, forKey: .listener)
        try keyed.encode(retry, forKey: .retry)
        try keyed.encode(endpoints, forKey: .endpoints)
        try keyed.encode(featureRules, forKey: .featureRules)
    }

    /// 全新安装的空壳：没有 Provider 候选，内建分流规则全部停用。
    public static let bootstrap = AppConfig(
        endpoints: [],
        featureRules: BuiltInFeatureRules.rules
    )

    public func normalizedBuiltInFeatureRules() -> AppConfig {
        var copy = self
        copy.featureRules = BuiltInFeatureRules.normalized(copy.featureRules)
        return copy
    }

    public mutating func normalizeBuiltInFeatureRules() {
        self = normalizedBuiltInFeatureRules()
    }

    public func endpoint(id endpointID: String) -> Endpoint? {
        endpoints.first { $0.id == endpointID }
    }

    /// Compatibility view for old panes. It is a computed virtual grouping,
    /// not a persisted concept; new UI should iterate `endpoints` directly.
    @available(*, deprecated, message: "Provider 池已移除；请直接使用 endpoints")
    public var pools: [Pool] {
        get { [Pool(id: "providers", name: "Providers", role: .primary, endpoints: endpoints)] }
        set { endpoints = newValue.flatMap(\.endpoints) }
    }

    @available(*, deprecated, message: "Provider 池已移除；请直接使用 endpoints")
    public var primaryPool: Pool? { pools.first }

    @available(*, deprecated, message: "Provider 池已移除；请直接使用 endpoints")
    public func pool(id poolID: String) -> Pool? {
        (poolID == "primary" || poolID == "providers") ? pools.first : nil
    }

    @available(*, deprecated, message: "Provider 池已移除；请直接使用 endpoints")
    public func pool(containingEndpoint endpointID: String) -> Pool? {
        endpoints.contains { $0.id == endpointID } ? pools.first : nil
    }
}

public enum ModelName {
    /// 清洗后的模型名 + 可选 CPA 风格 effort 后缀。
    public struct Parsed: Equatable, Sendable {
        public var baseName: String
        public var effort: ReasoningEffort?

        public init(baseName: String, effort: ReasoningEffort? = nil) {
            self.baseName = baseName
            self.effort = effort
        }
    }

    /// 解析模型名:去掉尾部 `[1m]` 与合法 `(effort)` 后缀。
    ///
    /// 格式与 CPA 对齐:`gpt-5.6-luna(high)` / `claude-opus-4-8(max)`。
    /// 仅当括号内是已知 effort 档位时才剥离,未知括号原样保留,避免误伤真实模型 id。
    /// 允许 `name (high)` 尾空格(会 trim),推荐无空格写法。
    public static func parse(_ model: String) -> Parsed {
        var cleaned = model.trimmingCharacters(in: .whitespacesAndNewlines)
        if let range = cleaned.range(of: #"\[[^\]]*\]\s*$"#, options: .regularExpression) {
            cleaned.removeSubrange(range)
            cleaned = cleaned.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        if let open = cleaned.lastIndex(of: "("), cleaned.hasSuffix(")") {
            let rawStart = cleaned.index(after: open)
            let rawEnd = cleaned.index(before: cleaned.endIndex)
            let raw = String(cleaned[rawStart..<rawEnd])
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .lowercased()
            if let effort = ReasoningEffort(rawValue: raw) {
                let base = String(cleaned[..<open])
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                return Parsed(baseName: base, effort: effort)
            }
        }
        return Parsed(baseName: cleaned, effort: nil)
    }

    public static func clean(_ model: String) -> String {
        parse(model).baseName
    }

    /// 「从已知模型添加」的默认上下文:opus/fable 系默认开 1M,其余不开。
    public static func defaultContext(for model: String) -> ContextMode {
        let lower = model.lowercased()
        return lower.contains("opus") || lower.contains("fable") ? .oneMillion : .standard
    }

    public static func reasoningEffort(from model: String) -> ReasoningEffort? {
        parse(model).effort
    }

}
