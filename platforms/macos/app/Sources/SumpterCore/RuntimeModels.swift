import Foundation

// 运行事件与内存统计快照。字段仍兼容旧 stats.json，但当前持久化后端是 runtime.sqlite3；
// 引擎移往 Rust sidecar(sumpterd)后,这些纯数据类型上移到 Core 供 UI 与 admin 客户端使用。
// wire 兼容:字段名、Date(secondsSinceReferenceDate)、可选字段省略 —— 与 sumpterd 的
// serde 输出/输入完全同构,见 rust/specs/spec-engine.md §5。

/// 事件阶段:进行中(已入流未结束)或已完成。nil 视作已完成,保证老 stats.json 兼容。
public enum RuntimeEventPhase: String, Codable, Sendable {
    case inFlight
    case completed
}

/// 最终结果独立于 HTTP 状态：响应头已经是 200 后仍可能断流失败。
public enum RuntimeEventOutcome: String, Codable, Sendable {
    case succeeded
    case failed
    case cancelled
}

public enum RuntimeFailureKind: String, Codable, Sendable {
    case responseTimeout = "response_timeout"
    case connectionFailed = "connection_failed"
    case invalidResponse = "invalid_response"
    case upstreamHTTPStatus = "upstream_http_status"
    case streamIdleTimeout = "stream_idle_timeout"
    case streamInterrupted = "stream_interrupted"
    case upstreamResponseIncomplete = "upstream_response_incomplete"
    case upstreamResponseFailed = "upstream_response_failed"
    case endpointsExhausted = "endpoints_exhausted"
    case clientCancelled = "client_cancelled"
    case clientRequestRejected = "client_request_rejected"
}

public enum RuntimeFailurePhase: String, Codable, Sendable {
    case beforeResponse = "before_response"
    case responseHeaders = "response_headers"
    case responseStream = "response_stream"
}

/// 入站客户端类型。由 sumpterd 按入站 UA + 入站方言判定,不猜请求体内容。
///
/// 排障时「谁在发这些请求」和「请求是什么用途」是两个独立维度:同一个
/// `classifier` 用途既可能来自 Claude Code 的自动模式,也可能来自别的客户端;
/// 同一个 Codex 既发普通请求也发工具请求。所以它与 `requestPurpose` 并列而不合并。
public enum ClientKind: String, Codable, CaseIterable, Equatable, Sendable {
    case claudeCode = "claude_code"
    case codex
    case grokBuild = "grok_build"
    /// 经 OpenAI 兼容层入站,但 UA 不是已知客户端。
    case openaiCompat = "openai_compat"
    /// Anthropic 入站且 UA 不是已知客户端(含缺 UA)。
    case unknown

    public var displayName: String {
        switch self {
        case .claudeCode:
            return "Claude Code"
        case .codex:
            return "Codex"
        case .grokBuild:
            return "Grok Build"
        case .openaiCompat:
            return "OpenAI 兼容客户端"
        case .unknown:
            return "未知客户端"
        }
    }
}

public struct ResponseUsage: Codable, Equatable, Sendable {
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var cacheReadInputTokens: Int?
    public var cacheCreationInputTokens: Int?
    public var reasoningTokens: Int?
}

public struct StreamTrace: Codable, Equatable, Sendable {
    public var chunkCount: Int?
    public var bytesReceived: Int?
    public var maxChunkGapMS: Int?
    public var lastChunkAtMS: Int?
    public var terminalEvent: String?
    public var usage: ResponseUsage?
    public var stopReason: String?
    public var websocketTrace: WebSocketTrace?

    public init(
        chunkCount: Int? = nil,
        bytesReceived: Int? = nil,
        maxChunkGapMS: Int? = nil,
        lastChunkAtMS: Int? = nil,
        terminalEvent: String? = nil,
        usage: ResponseUsage? = nil,
        stopReason: String? = nil,
        websocketTrace: WebSocketTrace? = nil
    ) {
        self.chunkCount = chunkCount
        self.bytesReceived = bytesReceived
        self.maxChunkGapMS = maxChunkGapMS
        self.lastChunkAtMS = lastChunkAtMS
        self.terminalEvent = terminalEvent
        self.usage = usage
        self.stopReason = stopReason
        self.websocketTrace = websocketTrace
    }
}

/// Bounded, protocol-level WebSocket relay metrics.  Close reasons and frame
/// payloads are intentionally absent; the Rust engine records only close
/// codes, side attribution and a safe error token.
public struct WebSocketTrace: Codable, Equatable, Sendable {
    public var handshakeStatus: Int?
    public var bytesSent: Int?
    public var bytesReceived: Int?
    public var clientMessageCount: Int?
    public var upstreamMessageCount: Int?
    public var closeCode: Int?
    public var clientCloseCode: Int?
    public var upstreamCloseCode: Int?
    public var closedBy: String?
    public var relayError: String?
    public var abnormalClose: Bool?
    public var attemptCount: Int?

    public init(
        handshakeStatus: Int? = nil,
        bytesSent: Int? = nil,
        bytesReceived: Int? = nil,
        clientMessageCount: Int? = nil,
        upstreamMessageCount: Int? = nil,
        closeCode: Int? = nil,
        clientCloseCode: Int? = nil,
        upstreamCloseCode: Int? = nil,
        closedBy: String? = nil,
        relayError: String? = nil,
        abnormalClose: Bool? = nil,
        attemptCount: Int? = nil
    ) {
        self.handshakeStatus = handshakeStatus
        self.bytesSent = bytesSent
        self.bytesReceived = bytesReceived
        self.clientMessageCount = clientMessageCount
        self.upstreamMessageCount = upstreamMessageCount
        self.closeCode = closeCode
        self.clientCloseCode = clientCloseCode
        self.upstreamCloseCode = upstreamCloseCode
        self.closedBy = closedBy
        self.relayError = relayError
        self.abnormalClose = abnormalClose
        self.attemptCount = attemptCount
    }
}

public struct CodexWorkspaceMetadata: Codable, Equatable, Sendable {
    public var associatedRemoteURLs: [String: String]
    public var latestGitCommitHash: String?
    public var hasChanges: Bool?

    private enum CodingKeys: String, CodingKey {
        case associatedRemoteURLs, latestGitCommitHash, hasChanges
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        associatedRemoteURLs = try c.decodeIfPresent([String: String].self, forKey: .associatedRemoteURLs) ?? [:]
        latestGitCommitHash = try c.decodeIfPresent(String.self, forKey: .latestGitCommitHash)
        hasChanges = try c.decodeIfPresent(Bool.self, forKey: .hasChanges)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        if !associatedRemoteURLs.isEmpty {
            try c.encode(associatedRemoteURLs, forKey: .associatedRemoteURLs)
        }
        try c.encodeIfPresent(latestGitCommitHash, forKey: .latestGitCommitHash)
        try c.encodeIfPresent(hasChanges, forKey: .hasChanges)
    }
}

public struct CodexToolSourceMetadata: Codable, Equatable, Sendable {
    public var kind: String
    public var serverName: String?
}

public struct CodexToolFunctionMetadata: Codable, Equatable, Sendable {
    public var name: String?
    public var direct: Bool?
    public var codeModeName: String?
    public var deferred: Bool?
    public var source: CodexToolSourceMetadata?
}

public struct CodexToolNamespaceMetadata: Codable, Equatable, Sendable {
    public var name: String?
    public var functions: [String: CodexToolFunctionMetadata]

    private enum CodingKeys: String, CodingKey {
        case name, functions
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        functions = try c.decodeIfPresent([String: CodexToolFunctionMetadata].self, forKey: .functions) ?? [:]
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(name, forKey: .name)
        if !functions.isEmpty {
            try c.encode(functions, forKey: .functions)
        }
    }
}

public struct CodexCompactionMetadata: Codable, Equatable, Sendable {
    public var trigger: String?
    public var reason: String?
    public var implementation: String?
    public var phase: String?
    public var strategy: String?
}

/// Safe, bounded Codex turn metadata emitted by the Rust sidecar.
/// 客户端用入站 `X-Sumpter-*` header 自称的项目归因。
///
/// 给 Claude Code 之类不上行 workspace 结构的客户端补项目维度用。可信度低于
/// `CodexMetadata`：值是客户端自称的，daemon 侧归因时排在 Codex workspace 之后。
/// 路径已由 daemon 脱敏到尾两段，不含完整绝对路径。
public struct ClientDeclaredMetadata: Codable, Equatable, Sendable {
    public var project: String?
    public var workspace: String?
    public var gitRemote: String?
    public var user: String?
    /// Source attribution retained by the local runtime SQLite detail payload.
    /// Display/analytics continue to use the compact fields above.
    public var sourceProject: String?
    public var sourceWorkspace: String?
    public var sourceUser: String?

    public init(
        project: String? = nil,
        workspace: String? = nil,
        gitRemote: String? = nil,
        user: String? = nil,
        sourceProject: String? = nil,
        sourceWorkspace: String? = nil,
        sourceUser: String? = nil
    ) {
        self.project = project
        self.workspace = workspace
        self.gitRemote = gitRemote
        self.user = user
        self.sourceProject = sourceProject
        self.sourceWorkspace = sourceWorkspace
        self.sourceUser = sourceUser
    }

    public var isEmpty: Bool {
        project == nil && workspace == nil && gitRemote == nil && user == nil
            && sourceProject == nil && sourceWorkspace == nil && sourceUser == nil
    }
}

public struct CodexMetadata: Codable, Equatable, Sendable {
    public var installationID: String?
    /// Unprojected, bounded installation ID kept for local session review.
    public var sourceInstallationID: String?
    public var sessionID: String?
    public var threadID: String?
    public var agentName: String?
    public var turnID: String?
    public var windowID: String?
    public var requestKind: String?
    public var forkedFromThreadID: String?
    public var parentThreadID: String?
    public var parentTurnID: String?
    public var rootTurnID: String?
    public var subagentHeader: String?
    public var subagentKind: String?
    public var threadSource: String?
    public var sandbox: String?
    public var sandboxMode: String?
    public var autoReviewEnabled: Bool?
    public var nodeReplAutoReviewRequired: Bool?
    public var nodeReplDisabled: Bool?
    public var turnStartedAtUnixMS: Int64?
    public var workspaces: [String: CodexWorkspaceMetadata]
    /// Unprojected, bounded workspace paths kept in runtime event details.
    public var sourceWorkspacePaths: [String]
    public var toolNamespacesInfo: [String: CodexToolNamespaceMetadata]
    public var compaction: CodexCompactionMetadata?
    public var extras: [String: String]
    public var originator: String?
    public var betaFeatures: String?
    public var memgenRequest: String?
    public var responsesLite: String?
    public var wsStreamRequestStartMS: Int64?
    public var sources: [String]
    public var redactedFields: [String]
    public var malformed: Bool
    public var truncated: Bool
    public var hasConflicts: Bool
    public var conflicts: [String]
    public var isSubagent: Bool
    public var parentThreadIDInferred: Bool

    private enum CodingKeys: String, CodingKey {
        case installationID, sourceInstallationID, sessionID, threadID, agentName, turnID, windowID, requestKind
        case forkedFromThreadID, parentThreadID, parentTurnID, rootTurnID
        case subagentHeader, subagentKind, threadSource, sandbox, sandboxMode
        case autoReviewEnabled, nodeReplAutoReviewRequired, nodeReplDisabled
        case turnStartedAtUnixMS, workspaces, sourceWorkspacePaths, toolNamespacesInfo, compaction, extras
        case originator, betaFeatures, memgenRequest, responsesLite, wsStreamRequestStartMS
        case sources, redactedFields, malformed, truncated, hasConflicts, conflicts
        case isSubagent, parentThreadIDInferred
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        installationID = try c.decodeIfPresent(String.self, forKey: .installationID)
        sourceInstallationID = try c.decodeIfPresent(String.self, forKey: .sourceInstallationID)
        sessionID = try c.decodeIfPresent(String.self, forKey: .sessionID)
        threadID = try c.decodeIfPresent(String.self, forKey: .threadID)
        agentName = try c.decodeIfPresent(String.self, forKey: .agentName)
        turnID = try c.decodeIfPresent(String.self, forKey: .turnID)
        windowID = try c.decodeIfPresent(String.self, forKey: .windowID)
        requestKind = try c.decodeIfPresent(String.self, forKey: .requestKind)
        forkedFromThreadID = try c.decodeIfPresent(String.self, forKey: .forkedFromThreadID)
        parentThreadID = try c.decodeIfPresent(String.self, forKey: .parentThreadID)
        parentTurnID = try c.decodeIfPresent(String.self, forKey: .parentTurnID)
        rootTurnID = try c.decodeIfPresent(String.self, forKey: .rootTurnID)
        subagentHeader = try c.decodeIfPresent(String.self, forKey: .subagentHeader)
        subagentKind = try c.decodeIfPresent(String.self, forKey: .subagentKind)
        threadSource = try c.decodeIfPresent(String.self, forKey: .threadSource)
        sandbox = try c.decodeIfPresent(String.self, forKey: .sandbox)
        sandboxMode = try c.decodeIfPresent(String.self, forKey: .sandboxMode)
        autoReviewEnabled = try c.decodeIfPresent(Bool.self, forKey: .autoReviewEnabled)
        nodeReplAutoReviewRequired = try c.decodeIfPresent(Bool.self, forKey: .nodeReplAutoReviewRequired)
        nodeReplDisabled = try c.decodeIfPresent(Bool.self, forKey: .nodeReplDisabled)
        turnStartedAtUnixMS = try c.decodeIfPresent(Int64.self, forKey: .turnStartedAtUnixMS)
        workspaces = try c.decodeIfPresent([String: CodexWorkspaceMetadata].self, forKey: .workspaces) ?? [:]
        sourceWorkspacePaths = try c.decodeIfPresent([String].self, forKey: .sourceWorkspacePaths) ?? []
        toolNamespacesInfo = try c.decodeIfPresent([String: CodexToolNamespaceMetadata].self, forKey: .toolNamespacesInfo) ?? [:]
        compaction = try c.decodeIfPresent(CodexCompactionMetadata.self, forKey: .compaction)
        extras = try c.decodeIfPresent([String: String].self, forKey: .extras) ?? [:]
        originator = try c.decodeIfPresent(String.self, forKey: .originator)
        betaFeatures = try c.decodeIfPresent(String.self, forKey: .betaFeatures)
        memgenRequest = try c.decodeIfPresent(String.self, forKey: .memgenRequest)
        responsesLite = try c.decodeIfPresent(String.self, forKey: .responsesLite)
        wsStreamRequestStartMS = try c.decodeIfPresent(Int64.self, forKey: .wsStreamRequestStartMS)
        sources = try c.decodeIfPresent([String].self, forKey: .sources) ?? []
        redactedFields = try c.decodeIfPresent([String].self, forKey: .redactedFields) ?? []
        malformed = try c.decodeIfPresent(Bool.self, forKey: .malformed) ?? false
        truncated = try c.decodeIfPresent(Bool.self, forKey: .truncated) ?? false
        hasConflicts = try c.decodeIfPresent(Bool.self, forKey: .hasConflicts) ?? false
        conflicts = try c.decodeIfPresent([String].self, forKey: .conflicts) ?? []
        isSubagent = try c.decodeIfPresent(Bool.self, forKey: .isSubagent) ?? false
        parentThreadIDInferred = try c.decodeIfPresent(Bool.self, forKey: .parentThreadIDInferred) ?? false
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(installationID, forKey: .installationID)
        try c.encodeIfPresent(sourceInstallationID, forKey: .sourceInstallationID)
        try c.encodeIfPresent(sessionID, forKey: .sessionID)
        try c.encodeIfPresent(threadID, forKey: .threadID)
        try c.encodeIfPresent(agentName, forKey: .agentName)
        try c.encodeIfPresent(turnID, forKey: .turnID)
        try c.encodeIfPresent(windowID, forKey: .windowID)
        try c.encodeIfPresent(requestKind, forKey: .requestKind)
        try c.encodeIfPresent(forkedFromThreadID, forKey: .forkedFromThreadID)
        try c.encodeIfPresent(parentThreadID, forKey: .parentThreadID)
        try c.encodeIfPresent(parentTurnID, forKey: .parentTurnID)
        try c.encodeIfPresent(rootTurnID, forKey: .rootTurnID)
        try c.encodeIfPresent(subagentHeader, forKey: .subagentHeader)
        try c.encodeIfPresent(subagentKind, forKey: .subagentKind)
        try c.encodeIfPresent(threadSource, forKey: .threadSource)
        try c.encodeIfPresent(sandbox, forKey: .sandbox)
        try c.encodeIfPresent(sandboxMode, forKey: .sandboxMode)
        try c.encodeIfPresent(autoReviewEnabled, forKey: .autoReviewEnabled)
        try c.encodeIfPresent(nodeReplAutoReviewRequired, forKey: .nodeReplAutoReviewRequired)
        try c.encodeIfPresent(nodeReplDisabled, forKey: .nodeReplDisabled)
        try c.encodeIfPresent(turnStartedAtUnixMS, forKey: .turnStartedAtUnixMS)
        if !workspaces.isEmpty { try c.encode(workspaces, forKey: .workspaces) }
        if !sourceWorkspacePaths.isEmpty { try c.encode(sourceWorkspacePaths, forKey: .sourceWorkspacePaths) }
        if !toolNamespacesInfo.isEmpty {
            try c.encode(toolNamespacesInfo, forKey: .toolNamespacesInfo)
        }
        try c.encodeIfPresent(compaction, forKey: .compaction)
        if !extras.isEmpty { try c.encode(extras, forKey: .extras) }
        try c.encodeIfPresent(originator, forKey: .originator)
        try c.encodeIfPresent(betaFeatures, forKey: .betaFeatures)
        try c.encodeIfPresent(memgenRequest, forKey: .memgenRequest)
        try c.encodeIfPresent(responsesLite, forKey: .responsesLite)
        try c.encodeIfPresent(wsStreamRequestStartMS, forKey: .wsStreamRequestStartMS)
        if !sources.isEmpty { try c.encode(sources, forKey: .sources) }
        if !redactedFields.isEmpty { try c.encode(redactedFields, forKey: .redactedFields) }
        try c.encode(malformed, forKey: .malformed)
        try c.encode(truncated, forKey: .truncated)
        try c.encode(hasConflicts, forKey: .hasConflicts)
        if !conflicts.isEmpty { try c.encode(conflicts, forKey: .conflicts) }
        try c.encode(isSubagent, forKey: .isSubagent)
        try c.encode(parentThreadIDInferred, forKey: .parentThreadIDInferred)
    }

    public var isEmpty: Bool {
        installationID == nil && sourceInstallationID == nil && sessionID == nil && threadID == nil && agentName == nil
            && turnID == nil
            && windowID == nil && requestKind == nil && forkedFromThreadID == nil
            && parentThreadID == nil && parentTurnID == nil && rootTurnID == nil
            && subagentHeader == nil && subagentKind == nil && threadSource == nil
            && sandbox == nil && sandboxMode == nil && autoReviewEnabled == nil
            && nodeReplAutoReviewRequired == nil && nodeReplDisabled == nil
            && turnStartedAtUnixMS == nil && workspaces.isEmpty && sourceWorkspacePaths.isEmpty
            && toolNamespacesInfo.isEmpty && compaction == nil && extras.isEmpty
            && originator == nil && betaFeatures == nil && memgenRequest == nil
            && responsesLite == nil && wsStreamRequestStartMS == nil
            && sources.isEmpty && redactedFields.isEmpty && conflicts.isEmpty
            && !malformed && !truncated && !hasConflicts && !isSubagent
            && !parentThreadIDInferred
    }
}

public struct RuntimeEvent: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var timestamp: Date
    public var kind: String
    public var poolID: String?
    public var endpointID: String?
    public var endpointName: String?
    public var upstreamHost: String?
    public var clientModel: String?
    /// 入站客户端类型。nil = 升级前的 stats.json 事件,或请求在读到入站信息前就被拒。
    public var clientKind: ClientKind?
    /// 入站路径确定的真实协议。旧事件或入站早期拒绝时为空。
    public var sourceFormat: ProviderProtocol?
    /// 本次请求实际选择的真实出站协议。不会写配置层的 Auto。
    public var targetFormat: ProviderProtocol?
    /// 本次请求走原生适配还是协议桥接。
    public var routeMode: RouteMode?
    public var upstreamModel: String?
    /// 路由定型后的逻辑模型。与 `upstreamModel` 分开，后者是实际发给上游的模型名。
    /// Rust 事件一直写入该字段；旧 macOS 壳此前漏解码，导致模型映射在界面上不可见。
    public var effectiveModel: String?
    public var statusCode: Int
    public var durationMS: Int
    public var failover: Bool
    public var message: String?
    /// 本轮响应流中实际发生的工具调用；只由协议事件填充，不代表请求声明的 tools。
    public var toolCalls: [String]?
    /// 本轮响应流的脱敏计时/字节摘要，不包含正文或请求头。
    public var streamTrace: StreamTrace?
    public var outcome: RuntimeEventOutcome?
    public var phase: RuntimeEventPhase?
    public var featureRuleID: String?
    public var failureDetail: String?
    public var failureKind: RuntimeFailureKind?
    public var failurePhase: RuntimeFailurePhase?
    /// 请求用途。nil 仅表示该事件来自升级前的 stats.json,或事件本身不是模型请求。
    public var requestPurpose: RequestPurpose?
    /// 一次客户端请求和它的全部上游尝试共享。
    public var requestID: String?
    /// 原始入站 HTTP method/path 与稳定意图标签，用于被拒请求排障。
    public var requestMethod: String?
    public var requestPath: String?
    public var routeIntent: String?
    /// 客户端提供的稳定会话标识（例如 Claude Code session header）。
    /// 只保存有界、无控制字符的标识，不保存会话正文。
    public var sessionID: String?
    /// 首字节耗时(ms):到上游响应头 accepted 为止。nil = 从未 accepted 或旧事件。
    ///
    /// **两类事件口径不同**:client 事件是客户端视角的总等待(含 failover 与上游重跑);
    /// upstream 事件只是该次尝试自身的响应头延迟。看错口径会把「换过一次入口」
    /// 误读成「这个入口很慢」。
    public var ttfbMS: Int?
    /// 实际生效的首响应/流空闲阈值。
    public var timeoutMS: Int?
    /// 真正从上游响应头收到的状态；连接前失败为 nil。
    public var upstreamStatusCode: Int?
    public var upstreamRequestID: String?
    public var codexMetadata: CodexMetadata?
    /// 客户端声明的项目归因；未配置或旧事件为 nil。
    public var clientDeclared: ClientDeclaredMetadata?
    /// 服务端算好的项目归因(投影列)。分页列表走 SQLite 投影快路径,不带
    /// codexMetadata / clientDeclared,只带这两个;缺了就会恒显示「未识别项目」。
    public var projectName: String?
    public var projectSource: String?
    /// Wrapper 采集的本机用户名，用于「本地(kkl)」；分页列表靠这个字段，不依赖 clientDeclared。
    public var localUser: String?
    /// Stable server-side Codex thread classification (for example `ambient`).
    public var codexThreadClass: String?
    /// `project`, `internal_feature`, or `unknown`; never inferred from IDs.
    public var attributionScope: String?

    private enum CodingKeys: String, CodingKey {
        case id, timestamp, kind, poolID, endpointID, endpointName, upstreamHost
        case clientModel, clientKind, sourceFormat, targetFormat, routeMode
        case upstreamModel, effectiveModel, statusCode, durationMS, failover
        case message, toolCalls, streamTrace, outcome, phase, featureRuleID
        case failureDetail, failureKind, failurePhase, requestPurpose, requestID
        case requestMethod, requestPath, routeIntent
        case sessionID, ttfbMS, timeoutMS, upstreamStatusCode, upstreamRequestID
        case codexMetadata, clientDeclared, projectName, projectSource, localUser, codexThreadClass, attributionScope
    }

    /// Runtime JSON has existed in three timestamp dialects over its lifetime:
    /// Apple reference seconds, Unix seconds, and Unix milliseconds.  Keep the
    /// wire contract backwards compatible while making the boundary explicit so
    /// a Unix value is never interpreted as Apple reference time (or multiplied
    /// twice).
    public enum Timestamp: Sendable {
        public static func date(from value: Double) -> Date {
            let magnitude = abs(value)
            if magnitude >= 1_000_000_000_000 {
                return Date(timeIntervalSince1970: value / 1_000)
            }
            if magnitude >= 1_000_000_000 {
                return Date(timeIntervalSince1970: value)
            }
            return Date(timeIntervalSinceReferenceDate: value)
        }

        public static func appleReferenceSeconds(_ date: Date) -> Double {
            date.timeIntervalSinceReferenceDate
        }
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        timestamp = Timestamp.date(from: try c.decode(Double.self, forKey: .timestamp))
        kind = try c.decode(String.self, forKey: .kind)
        poolID = try c.decodeIfPresent(String.self, forKey: .poolID)
        endpointID = try c.decodeIfPresent(String.self, forKey: .endpointID)
        endpointName = try c.decodeIfPresent(String.self, forKey: .endpointName)
        upstreamHost = try c.decodeIfPresent(String.self, forKey: .upstreamHost)
        clientModel = try c.decodeIfPresent(String.self, forKey: .clientModel)
        clientKind = try c.decodeIfPresent(ClientKind.self, forKey: .clientKind)
        sourceFormat = try c.decodeIfPresent(ProviderProtocol.self, forKey: .sourceFormat)
        targetFormat = try c.decodeIfPresent(ProviderProtocol.self, forKey: .targetFormat)
        routeMode = try c.decodeIfPresent(RouteMode.self, forKey: .routeMode)
        upstreamModel = try c.decodeIfPresent(String.self, forKey: .upstreamModel)
        effectiveModel = try c.decodeIfPresent(String.self, forKey: .effectiveModel)
        statusCode = try c.decode(Int.self, forKey: .statusCode)
        durationMS = try c.decode(Int.self, forKey: .durationMS)
        failover = try c.decode(Bool.self, forKey: .failover)
        message = try c.decodeIfPresent(String.self, forKey: .message)
        toolCalls = try c.decodeIfPresent([String].self, forKey: .toolCalls)
        streamTrace = try c.decodeIfPresent(StreamTrace.self, forKey: .streamTrace)
        outcome = try c.decodeIfPresent(RuntimeEventOutcome.self, forKey: .outcome)
        phase = try c.decodeIfPresent(RuntimeEventPhase.self, forKey: .phase)
        featureRuleID = try c.decodeIfPresent(String.self, forKey: .featureRuleID)
        failureDetail = try c.decodeIfPresent(String.self, forKey: .failureDetail)
        failureKind = try c.decodeIfPresent(RuntimeFailureKind.self, forKey: .failureKind)
        failurePhase = try c.decodeIfPresent(RuntimeFailurePhase.self, forKey: .failurePhase)
        requestPurpose = try c.decodeIfPresent(RequestPurpose.self, forKey: .requestPurpose)
        requestID = try c.decodeIfPresent(String.self, forKey: .requestID)
        requestMethod = try c.decodeIfPresent(String.self, forKey: .requestMethod)
        requestPath = try c.decodeIfPresent(String.self, forKey: .requestPath)
        routeIntent = try c.decodeIfPresent(String.self, forKey: .routeIntent)
        sessionID = try c.decodeIfPresent(String.self, forKey: .sessionID)
        ttfbMS = try c.decodeIfPresent(Int.self, forKey: .ttfbMS)
        timeoutMS = try c.decodeIfPresent(Int.self, forKey: .timeoutMS)
        upstreamStatusCode = try c.decodeIfPresent(Int.self, forKey: .upstreamStatusCode)
        upstreamRequestID = try c.decodeIfPresent(String.self, forKey: .upstreamRequestID)
        codexMetadata = try c.decodeIfPresent(CodexMetadata.self, forKey: .codexMetadata)
        clientDeclared = try c.decodeIfPresent(ClientDeclaredMetadata.self, forKey: .clientDeclared)
        projectName = try c.decodeIfPresent(String.self, forKey: .projectName)
        projectSource = try c.decodeIfPresent(String.self, forKey: .projectSource)
        localUser = try c.decodeIfPresent(String.self, forKey: .localUser)
        codexThreadClass = try c.decodeIfPresent(String.self, forKey: .codexThreadClass)
        attributionScope = try c.decodeIfPresent(String.self, forKey: .attributionScope)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(Timestamp.appleReferenceSeconds(timestamp), forKey: .timestamp)
        try c.encode(kind, forKey: .kind)
        // poolID is a legacy routing/configuration field. Decode it for old
        // stats files, but keep it out of new runtime/detail JSON.
        try c.encodeIfPresent(endpointID, forKey: .endpointID)
        try c.encodeIfPresent(endpointName, forKey: .endpointName)
        try c.encodeIfPresent(upstreamHost, forKey: .upstreamHost)
        try c.encodeIfPresent(clientModel, forKey: .clientModel)
        try c.encodeIfPresent(clientKind, forKey: .clientKind)
        try c.encodeIfPresent(sourceFormat, forKey: .sourceFormat)
        try c.encodeIfPresent(targetFormat, forKey: .targetFormat)
        try c.encodeIfPresent(routeMode, forKey: .routeMode)
        try c.encodeIfPresent(upstreamModel, forKey: .upstreamModel)
        try c.encodeIfPresent(effectiveModel, forKey: .effectiveModel)
        try c.encode(statusCode, forKey: .statusCode)
        try c.encode(durationMS, forKey: .durationMS)
        try c.encode(failover, forKey: .failover)
        try c.encodeIfPresent(message, forKey: .message)
        try c.encodeIfPresent(toolCalls, forKey: .toolCalls)
        try c.encodeIfPresent(streamTrace, forKey: .streamTrace)
        try c.encodeIfPresent(outcome, forKey: .outcome)
        try c.encodeIfPresent(phase, forKey: .phase)
        try c.encodeIfPresent(featureRuleID, forKey: .featureRuleID)
        try c.encodeIfPresent(failureDetail, forKey: .failureDetail)
        try c.encodeIfPresent(failureKind, forKey: .failureKind)
        try c.encodeIfPresent(failurePhase, forKey: .failurePhase)
        try c.encodeIfPresent(requestPurpose, forKey: .requestPurpose)
        try c.encodeIfPresent(requestID, forKey: .requestID)
        try c.encodeIfPresent(requestMethod, forKey: .requestMethod)
        try c.encodeIfPresent(requestPath, forKey: .requestPath)
        try c.encodeIfPresent(routeIntent, forKey: .routeIntent)
        try c.encodeIfPresent(sessionID, forKey: .sessionID)
        try c.encodeIfPresent(ttfbMS, forKey: .ttfbMS)
        try c.encodeIfPresent(timeoutMS, forKey: .timeoutMS)
        try c.encodeIfPresent(upstreamStatusCode, forKey: .upstreamStatusCode)
        try c.encodeIfPresent(upstreamRequestID, forKey: .upstreamRequestID)
        try c.encodeIfPresent(codexMetadata, forKey: .codexMetadata)
        try c.encodeIfPresent(clientDeclared, forKey: .clientDeclared)
        try c.encodeIfPresent(projectName, forKey: .projectName)
        try c.encodeIfPresent(projectSource, forKey: .projectSource)
        try c.encodeIfPresent(localUser, forKey: .localUser)
        try c.encodeIfPresent(codexThreadClass, forKey: .codexThreadClass)
        try c.encodeIfPresent(attributionScope, forKey: .attributionScope)
    }

    public var isInFlight: Bool { phase == .inFlight }
    /// 兼容曾把已收到响应头的 client 事件写成 statusCode=0 的旧 stats.json。
    /// 新事件始终直接使用 statusCode；仅在 0 时回退已明确记录的上游状态。
    public var effectiveHTTPStatusCode: Int {
        statusCode == 0 ? (upstreamStatusCode ?? statusCode) : statusCode
    }
    public var isSucceeded: Bool {
        guard !isInFlight else { return false }
        return outcome.map { $0 == .succeeded } ?? (200..<400).contains(effectiveHTTPStatusCode)
    }
    public var isFailed: Bool {
        guard !isInFlight else { return false }
        return outcome.map { $0 == .failed }
            ?? (effectiveHTTPStatusCode != 499 && !(200..<400).contains(effectiveHTTPStatusCode))
    }
    public var isCancelled: Bool {
        outcome.map { $0 == .cancelled } ?? (effectiveHTTPStatusCode == 499)
    }

    public init(
        id: String = UUID().uuidString,
        timestamp: Date = Date(),
        kind: String,
        poolID: String? = nil,
        endpointID: String? = nil,
        endpointName: String? = nil,
        upstreamHost: String? = nil,
        clientModel: String? = nil,
        clientKind: ClientKind? = nil,
        sourceFormat: ProviderProtocol? = nil,
        targetFormat: ProviderProtocol? = nil,
        routeMode: RouteMode? = nil,
        upstreamModel: String? = nil,
        effectiveModel: String? = nil,
        statusCode: Int,
        durationMS: Int,
        failover: Bool = false,
        message: String? = nil,
        toolCalls: [String]? = nil,
        streamTrace: StreamTrace? = nil,
        outcome: RuntimeEventOutcome? = nil,
        phase: RuntimeEventPhase? = nil,
        featureRuleID: String? = nil,
        failureDetail: String? = nil,
        failureKind: RuntimeFailureKind? = nil,
        failurePhase: RuntimeFailurePhase? = nil,
        requestPurpose: RequestPurpose? = nil,
        requestID: String? = nil,
        requestMethod: String? = nil,
        requestPath: String? = nil,
        routeIntent: String? = nil,
        sessionID: String? = nil,
        ttfbMS: Int? = nil,
        timeoutMS: Int? = nil,
        upstreamStatusCode: Int? = nil,
        upstreamRequestID: String? = nil,
        codexMetadata: CodexMetadata? = nil,
        clientDeclared: ClientDeclaredMetadata? = nil,
        projectName: String? = nil,
        projectSource: String? = nil,
        localUser: String? = nil,
        codexThreadClass: String? = nil,
        attributionScope: String? = nil
    ) {
        self.id = id
        self.timestamp = timestamp
        self.kind = kind
        self.poolID = poolID
        self.endpointID = endpointID
        self.endpointName = endpointName
        self.upstreamHost = upstreamHost
        self.clientModel = clientModel
        self.clientKind = clientKind
        self.sourceFormat = sourceFormat
        self.targetFormat = targetFormat
        self.routeMode = routeMode
        self.upstreamModel = upstreamModel
        self.effectiveModel = effectiveModel
        self.statusCode = statusCode
        self.durationMS = durationMS
        self.failover = failover
        self.message = message
        self.toolCalls = toolCalls
        self.streamTrace = streamTrace
        self.outcome = outcome
        self.phase = phase
        self.featureRuleID = featureRuleID
        self.failureDetail = failureDetail
        self.failureKind = failureKind
        self.failurePhase = failurePhase
        self.requestPurpose = requestPurpose
        self.requestID = requestID
        self.requestMethod = requestMethod
        self.requestPath = requestPath
        self.routeIntent = routeIntent
        self.sessionID = sessionID
        self.ttfbMS = ttfbMS
        self.timeoutMS = timeoutMS
        self.upstreamStatusCode = upstreamStatusCode
        self.upstreamRequestID = upstreamRequestID
        self.projectName = projectName
        self.projectSource = projectSource
        self.localUser = localUser
        self.codexThreadClass = codexThreadClass
        self.attributionScope = attributionScope
        self.codexMetadata = codexMetadata
        self.clientDeclared = clientDeclared
    }
}

/// 运行事件表的类型筛选。
///
/// 「客户端」与「上游」是两种粒度:一次客户端请求恒定一条 client 事件,而每次上游尝试各一条
/// upstream 事件(failover 时一次请求会有多条)。合并会丢掉 failover 链,所以用筛选而不是去重。
public enum RuntimeEventKindFilter: String, CaseIterable, Equatable, Sendable {
    case all
    case client
    case upstream

    public var title: String {
        switch self {
        case .all:
            "全部"
        case .client:
            "客户端"
        case .upstream:
            "上游"
        }
    }
}

public extension RuntimeEvent {
    /// 事件所属的请求键。新事件用共享 `requestID` 聚合；旧事件没有该字段时退回自身 id。
    var requestGroupID: String { requestID ?? id }

    /// 返回选中事件对应的完整请求链，包含一条 client 事件和所有 upstream 尝试。
    /// 事件时间戳在流结束时会被原地更新，因此这里稳定按时间倒序、同时间保持原序。
    static func requestChain(_ events: [RuntimeEvent], selectedID: String) -> [RuntimeEvent] {
        guard let selected = events.first(where: { $0.id == selectedID }) else { return [] }
        let group = selected.requestGroupID
        return events
            .filter { $0.requestGroupID == group }
            .enumerated()
            .sorted { lhs, rhs in
                if lhs.element.timestamp == rhs.element.timestamp {
                    return lhs.offset < rhs.offset
                }
                return lhs.element.timestamp > rhs.element.timestamp
            }
            .map(\.element)
    }

    /// 展示用:按类型过滤并按时间降序。
    ///
    /// 引擎侧 upsert 为保住「流式进行中 → 完成」的原地更新语义不会移动行位置,
    /// 所以流式请求结束后时间戳变新、行却还钉在旧位置 —— 排序放在展示层统一做。
    /// 时间戳相同的事件保持原数组顺序(即插入顺序,新的在前)。
    static func ordered(_ events: [RuntimeEvent], filter: RuntimeEventKindFilter) -> [RuntimeEvent] {
        events
            .filter { filter == .all || $0.kind == filter.rawValue }
            .enumerated()
            .sorted { lhs, rhs in
                if lhs.element.timestamp == rhs.element.timestamp {
                    return lhs.offset < rhs.offset
                }
                return lhs.element.timestamp > rhs.element.timestamp
            }
            .map(\.element)
    }

    /// 按事件类型各留最近 `perKindLimit` 条(入参按新 → 旧排列,返回值保持原顺序)。
    /// 进行中的事件优先保留但单独计数、同样封顶。
    static func trimmed(_ events: [RuntimeEvent], perKindLimit: Int) -> [RuntimeEvent] {
        guard events.count > perKindLimit else {
            return events
        }
        var counts: [String: Int] = [:]
        var inFlightCounts: [String: Int] = [:]
        return events.filter { event in
            if event.isInFlight {
                let count = inFlightCounts[event.kind, default: 0]
                guard count < perKindLimit else {
                    return false
                }
                inFlightCounts[event.kind] = count + 1
                return true
            }
            let count = counts[event.kind, default: 0]
            guard count < perKindLimit else {
                return false
            }
            counts[event.kind] = count + 1
            return true
        }
    }
}

public struct RuntimeSnapshot: Codable, Equatable, Sendable {
    public var clientRequests: Int
    public var clientSuccesses: Int
    public var clientFailures: Int
    public var upstreamAttempts: Int
    public var upstreamSuccesses: Int
    public var upstreamFailures: Int
    public var failovers: Int
    public var recentEvents: [RuntimeEvent]

    public init(
        clientRequests: Int = 0,
        clientSuccesses: Int = 0,
        clientFailures: Int = 0,
        upstreamAttempts: Int = 0,
        upstreamSuccesses: Int = 0,
        upstreamFailures: Int = 0,
        failovers: Int = 0,
        recentEvents: [RuntimeEvent] = []
    ) {
        self.clientRequests = clientRequests
        self.clientSuccesses = clientSuccesses
        self.clientFailures = clientFailures
        self.upstreamAttempts = upstreamAttempts
        self.upstreamSuccesses = upstreamSuccesses
        self.upstreamFailures = upstreamFailures
        self.failovers = failovers
        self.recentEvents = recentEvents
    }
}

public struct ProxyNotification: Codable, Equatable, Sendable {
    public var title: String
    public var message: String
    public var sound: String?
    public var type: String?

    public init(title: String = "Claude Code", message: String, sound: String? = nil, type: String? = nil) {
        self.title = title
        self.message = message
        self.sound = sound
        self.type = type
    }
}
