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

public struct RequestPolicy: Codable, Equatable, Sendable {
    public var maxTotalAttempts = 0
    public var streamProtection = "complete"
    public var streamBufferMiB = 16
    public var streamBufferChunks = 8192
    public var streamBufferSeconds = 0
    public var maxBufferedMiB = 128
    public var global = RequestLimits()
    public var endpoints: [String: EndpointRequestPolicy] = [:]
    public init() {}
    enum CodingKeys: String, CodingKey {
        case maxTotalAttempts, streamProtection, streamBufferChunks, streamBufferSeconds, global, endpoints
        case streamBufferMiB = "streamBufferMib"
        case maxBufferedMiB = "maxBufferedMib"
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        maxTotalAttempts = try c.decodeIfPresent(Int.self, forKey: .maxTotalAttempts) ?? 0
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
         "attempt_budget": "达到总尝试上限", "backoff": "退避", "wait_completed": "等待结束",
         "same_group_capacity": "优先选择同组空闲入口", "session_group": "从当前会话调度组开始",
         "idle_timeout": "缓冲期间流空闲超时", "missing_terminal": "上游未发送终态",
         "retryable_terminal": "可重试的协议错误", "failed": "失败", "finished": "结束", "cancelled": "已取消"][detail] ?? detail
    }
}
