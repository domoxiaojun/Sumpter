import Foundation

public struct RequestLimits: Codable, Equatable, Sendable {
    public var maxConcurrent = 0
    public var maxQueued = 128
    public var queueTimeoutSeconds = 30
    public var requestsPerMinute = 0
    public init() {}
    enum CodingKeys: String, CodingKey { case maxConcurrent, maxQueued, queueTimeoutSeconds, requestsPerMinute }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        maxConcurrent = try c.decodeIfPresent(Int.self, forKey: .maxConcurrent) ?? 0
        maxQueued = try c.decodeIfPresent(Int.self, forKey: .maxQueued) ?? 128
        queueTimeoutSeconds = try c.decodeIfPresent(Int.self, forKey: .queueTimeoutSeconds) ?? 30
        requestsPerMinute = try c.decodeIfPresent(Int.self, forKey: .requestsPerMinute) ?? 0
    }
}

public struct CompatibilityProfile: Codable, Equatable, Sendable {
    public var strictTools: Bool?
    public var grammarTools: Bool?
    public var developerRole: Bool?
    public var structuredOutput: Bool?
    public init() {}
}

public struct EndpointRequestPolicy: Codable, Equatable, Sendable {
    public var limits = RequestLimits()
    public var models: [String: CompatibilityProfile] = [:]
    public var protocolModels: [String: [String: CompatibilityProfile]] = [:]
    public init() {}
    enum CodingKeys: String, CodingKey { case limits, models, protocolModels }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        limits = try c.decodeIfPresent(RequestLimits.self, forKey: .limits) ?? RequestLimits()
        models = try c.decodeIfPresent([String: CompatibilityProfile].self, forKey: .models) ?? [:]
        protocolModels = try c.decodeIfPresent([String: [String: CompatibilityProfile]].self, forKey: .protocolModels) ?? [:]
    }
}

public struct IngressPolicy: Codable, Equatable, Sendable {
    public var maxBodyMiB = 256
    public var maxDecodedBodyMiB = 256
    public var maxBufferedMiB = 1024
    public var maxPreprocessing = 4
    public var maxDecompressions = 2
    public var bodyIdleTimeoutSeconds = 30
    public var bodyReadTimeoutSeconds = 300
    public init() {}
    enum CodingKeys: String, CodingKey {
        case maxBodyMiB = "maxBodyMib"
        case maxDecodedBodyMiB = "maxDecodedBodyMib"
        case maxBufferedMiB = "maxBufferedMib"
        case maxPreprocessing
        case maxDecompressions
        case bodyIdleTimeoutSeconds
        case bodyReadTimeoutSeconds
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        maxBodyMiB = try c.decodeIfPresent(Int.self, forKey: .maxBodyMiB) ?? 256
        maxDecodedBodyMiB = try c.decodeIfPresent(Int.self, forKey: .maxDecodedBodyMiB) ?? 256
        maxBufferedMiB = try c.decodeIfPresent(Int.self, forKey: .maxBufferedMiB) ?? 1024
        maxPreprocessing = try c.decodeIfPresent(Int.self, forKey: .maxPreprocessing) ?? 4
        maxDecompressions = try c.decodeIfPresent(Int.self, forKey: .maxDecompressions) ?? 2
        bodyIdleTimeoutSeconds = try c.decodeIfPresent(Int.self, forKey: .bodyIdleTimeoutSeconds) ?? 30
        bodyReadTimeoutSeconds = try c.decodeIfPresent(Int.self, forKey: .bodyReadTimeoutSeconds) ?? 300
        try validate()
    }
    public func validate() throws {
        guard (1...1024).contains(maxBodyMiB) else { throw InputValidationError("HTTP 单请求体（MiB）必须为 1–1024") }
        guard (1...1024).contains(maxDecodedBodyMiB) else { throw InputValidationError("解压输出上限（MiB）必须为 1–1024") }
        guard (1...16384).contains(maxBufferedMiB) else { throw InputValidationError("入站受控缓冲总预算（MiB）必须为 1–16384") }
        guard (1...64).contains(maxPreprocessing) else { throw InputValidationError("同时读体、解压、解析必须为 1–64") }
        guard (1...64).contains(maxDecompressions) else { throw InputValidationError("同时解压必须为 1–64") }
        guard (1...3600).contains(bodyIdleTimeoutSeconds) else { throw InputValidationError("读体空闲时限（秒）必须为 1–3600") }
        guard (1...86400).contains(bodyReadTimeoutSeconds) else { throw InputValidationError("读体总时限（秒）必须为 1–86400") }
        guard maxBufferedMiB >= maxBodyMiB + maxDecodedBodyMiB else { throw InputValidationError("入站总缓冲必须容纳原始体上限与解压输出上限之和") }
        guard bodyReadTimeoutSeconds >= bodyIdleTimeoutSeconds else { throw InputValidationError("读体总时限不得小于空闲时限") }
    }
}

public struct WebSocketPolicy: Codable, Equatable, Sendable {
    public var maxFrameMiB = 256
    public var maxMessageMiB = 256
    public var maxConnections = 4
    public var firstMessageTimeoutSeconds = 30
    public init() {}
    enum CodingKeys: String, CodingKey {
        case maxFrameMiB = "maxFrameMib"
        case maxMessageMiB = "maxMessageMib"
        case maxConnections
        case firstMessageTimeoutSeconds
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        maxFrameMiB = try c.decodeIfPresent(Int.self, forKey: .maxFrameMiB) ?? 256
        maxMessageMiB = try c.decodeIfPresent(Int.self, forKey: .maxMessageMiB) ?? 256
        maxConnections = try c.decodeIfPresent(Int.self, forKey: .maxConnections) ?? 4
        firstMessageTimeoutSeconds = try c.decodeIfPresent(Int.self, forKey: .firstMessageTimeoutSeconds) ?? 30
        try validate()
    }
    public func validate() throws {
        guard (1...1024).contains(maxFrameMiB) else { throw InputValidationError("单帧上限（MiB）必须为 1–1024") }
        guard (1...1024).contains(maxMessageMiB) else { throw InputValidationError("累计分片消息上限（MiB）必须为 1–1024") }
        guard (1...1024).contains(maxConnections) else { throw InputValidationError("业务连接上限必须为 1–1024") }
        guard (1...3600).contains(firstMessageTimeoutSeconds) else { throw InputValidationError("首个业务消息期限（秒）必须为 1–3600") }
        guard maxFrameMiB <= maxMessageMiB else { throw InputValidationError("WebSocket 帧上限不得超过消息上限") }
    }
}

public struct RequestPolicy: Codable, Equatable, Sendable {
    public var ingress = IngressPolicy()
    public var websocket = WebSocketPolicy()
    public var maxTotalAttempts = 0
    public var responseWaitTimeoutSeconds = 0
    public var maxConsecutiveNoResponse = 3
    public var noResponseCooldownSeconds = 300
    public var streamProtection = "complete"
    public var streamBufferMiB = 16
    public var streamBufferChunks = 8192
    public var streamBufferSeconds = 0
    public var maxBufferedMiB = 128
    public var global = RequestLimits()
    public var endpoints: [String: EndpointRequestPolicy] = [:]
    public init() {}
    enum CodingKeys: String, CodingKey {
        case ingress, websocket
        case maxTotalAttempts, responseWaitTimeoutSeconds, streamProtection, streamBufferChunks, streamBufferSeconds, global, endpoints
        case maxConsecutiveNoResponse, noResponseCooldownSeconds
        case streamBufferMiB = "streamBufferMib"
        case maxBufferedMiB = "maxBufferedMib"
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ingress = try c.decodeIfPresent(IngressPolicy.self, forKey: .ingress) ?? IngressPolicy()
        websocket = try c.decodeIfPresent(WebSocketPolicy.self, forKey: .websocket) ?? WebSocketPolicy()
        maxTotalAttempts = try c.decodeIfPresent(Int.self, forKey: .maxTotalAttempts) ?? 0
        responseWaitTimeoutSeconds = try c.decodeIfPresent(Int.self, forKey: .responseWaitTimeoutSeconds) ?? 0
        maxConsecutiveNoResponse = try c.decodeIfPresent(Int.self, forKey: .maxConsecutiveNoResponse) ?? 3
        noResponseCooldownSeconds = try c.decodeIfPresent(Int.self, forKey: .noResponseCooldownSeconds) ?? 300
        streamProtection = try c.decodeIfPresent(String.self, forKey: .streamProtection) ?? "complete"
        streamBufferMiB = try c.decodeIfPresent(Int.self, forKey: .streamBufferMiB) ?? 16
        streamBufferChunks = try c.decodeIfPresent(Int.self, forKey: .streamBufferChunks) ?? 8192
        streamBufferSeconds = try c.decodeIfPresent(Int.self, forKey: .streamBufferSeconds) ?? 0
        maxBufferedMiB = try c.decodeIfPresent(Int.self, forKey: .maxBufferedMiB) ?? 128
        global = try c.decodeIfPresent(RequestLimits.self, forKey: .global) ?? RequestLimits()
        endpoints = try c.decodeIfPresent([String: EndpointRequestPolicy].self, forKey: .endpoints) ?? [:]
    }
}

public struct RequestTrace: Codable, Equatable, Sendable {
    public var steps: [RequestTraceStep]
    public var omitted: Int
}

public struct RequestTraceStep: Codable, Equatable, Sendable {
    public var atMs: Int
    public var action: String
    public var endpointId: String?
    public var detail: String
    public var durationMs: Int?
    public var label: String {
        ["plan": "规划", "candidate": "候选", "affinity": "会话归属", "rule": "分流规则", "queue": "排队", "attempt": "发送上游", "headers": "响应头",
         "buffer": "缓冲", "failure": "失败分类", "skip": "跳过", "admission_rejected": "本地限流",
         "retry_scheduled": "计划重试", "retry_wait": "重试等待", "stop": "停止", "first_chunk": "首个响应块",
         "complete": "完成"][action] ?? action
    }
    public var reason: String {
        ["configured_candidates": "按配置选择候选", "native": "原生转发", "translated": "协议转换",
         "admitted": "获得发送名额", "send": "开始发送", "received": "已收到响应头", "first_chunk_received": "已收到响应块", "transport_error": "连接失败",
         "terminal": "已收到终态", "first_output": "开始实时转发", "time_budget": "达到缓冲时限",
         "global_buffer_budget": "达到总缓冲额度", "chunk_budget": "达到响应块数上限", "byte_budget": "达到单流缓冲额度",
         "quota_exhausted": "额度耗尽", "credential": "鉴权错误", "model_unsupported": "模型不支持",
         "rate_limit": "暂时限流", "upstream_error": "上游错误", "queue_full": "队列已满", "queue_timeout": "排队超时",
         "attempt_budget": "达到总尝试上限", "response_wait_timeout": "代理响应等待达到上限", "backoff": "退避", "wait_completed": "等待结束",
         "no_response_retry_exhausted": "连续未收到响应，已停止向该入口重试",
         "same_group_capacity": "优先选择同组空闲入口", "session_group": "从当前会话调度组开始",
         "idle_timeout": "缓冲期间流空闲超时", "missing_terminal": "上游未发送终态",
         "retryable_terminal": "可重试的协议错误", "failed": "失败", "finished": "结束", "cancelled": "已取消"][detail] ?? detail
    }
}
