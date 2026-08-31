import Foundation

/// 运行事件的展示辅助(纯逻辑,可单测)。
/// 把引擎记录的原始错误串转成可读中文;原始串仍保留在事件里供详情查看。
public enum RuntimeEventPresentation {
    /// 归因来源。rawValue 与服务端 analytics 的 `projectSource` 及 Linux WebUI 的
    /// `projectSourceLabel` 同一套 token,同一请求在用量页和事件详情里不会读成两个来源。
    public enum ProjectSource: String, Equatable, Sendable {
        case workspaceLocal = "workspace_local"
        case workspaceRemoteFallback = "workspace_remote_fallback"
        case workspaceUnidentified = "workspace_unidentified"
        case multipleWorkspaces = "multiple_workspaces"
        case clientDeclared = "client_declared"
        case missingWorkspaceMetadata = "missing_workspace_metadata"
        case internalFeature = "internal_feature"

        /// 事件详情「项目来源」行的文案。UsagePane 的维度表带「来源：」前缀,
        /// 这里由 InfoRow 标题承担,所以只给值。
        public var label: String {
            switch self {
            case .workspaceLocal: "本地项目"
            case .workspaceRemoteFallback: "远程仓库回退"
            case .workspaceUnidentified: "未识别"
            case .multipleWorkspaces: "多个项目，未拆分"
            case .clientDeclared: "客户端声明"
            case .missingWorkspaceMetadata: "未记录"
            case .internalFeature: "后台功能"
            }
        }
    }

    /// 事件的项目归因:显示名 + 来源 + 补充证据(工作区尾段 / git remote)。
    public struct ProjectContext: Equatable, Sendable {
        public let name: String
        public let source: ProjectSource
        public let detail: String?

        public init(name: String, source: ProjectSource, detail: String? = nil) {
            self.name = name
            self.source = source
            self.detail = detail
        }
    }

    /// 合成的「未识别」桶名,与 analytics 的 `unidentified_project` 行对应。
    public static let unidentifiedProjectName = "未识别项目"

    /// Project attribution exists only on client requests. Upstream attempts and
    /// notifications must not be assigned to the synthetic unidentified bucket.
    ///
    /// Codex 的结构化 workspace 优先于 `X-Kekulv-*` 声明:前者是客户端采集的,后者只是
    /// 客户端自称的。这个优先级与 Linux WebUI `helpers.js:eventProjectContext` 和服务端
    /// `runtime_store` 的 analytics 归因一致,不要单独调。
    public static func projectContext(
        eventKind: String,
        metadata: CodexMetadata?,
        declared: ClientDeclaredMetadata?,
        projectedName: String? = nil,
        projectedSource: String? = nil,
        attributionScope: String? = nil
    ) -> ProjectContext? {
        guard eventKind == "client" else { return nil }

        if attributionScope == ProjectSource.internalFeature.rawValue
            || projectedSource == ProjectSource.internalFeature.rawValue {
            return ProjectContext(
                name: "后台功能",
                source: .internalFeature,
                detail: nil
            )
        }

        // 分页列表走服务端投影快路径,不带 codexMetadata / clientDeclared,只带算好的
        // projectName + projectSource。有它们就直接用——优先级已由服务端 project_identity
        // 统一决定;否则(SSE 推送、单事件详情、旧 daemon)按下面的完整字段自行推导。
        if let projected = nonEmpty(projectedName) {
            let source = ProjectSource(rawValue: nonEmpty(projectedSource) ?? "")
                ?? .missingWorkspaceMetadata
            return ProjectContext(
                name: projected == "unidentified_project" ? unidentifiedProjectName : projected,
                source: source
            )
        }

        if let metadata, !metadata.workspaces.isEmpty {
            let paths = metadata.workspaces.keys.sorted()
            let projects = paths.map { workspaceProjectName(path: $0, workspace: metadata.workspaces[$0]) }
            if paths.count > 1 {
                return ProjectContext(
                    name: projects.joined(separator: " + "),
                    source: .multipleWorkspaces
                )
            }
            let path = paths[0]
            let remotes = metadata.workspaces[path]?.associatedRemoteURLs.values
                .filter { !$0.isEmpty } ?? []
            let source: ProjectSource = path.isEmpty
                ? (remotes.isEmpty ? .workspaceUnidentified : .workspaceRemoteFallback)
                : .workspaceLocal
            return ProjectContext(
                name: projects[0],
                source: source,
                detail: remotes.first ?? (path.isEmpty ? nil : path)
            )
        }

        // Codex 没给结构化 workspace 时才看客户端自称的归因(Claude Code 走这条)。
        if let declared, !declared.isEmpty {
            let project = nonEmpty(declared.project)
            let workspace = nonEmpty(declared.workspace)
            let remote = nonEmpty(declared.gitRemote)
            if let name = project ?? workspace ?? remote {
                let detail = [workspace, remote]
                    .compactMap { $0 }
                    .filter { $0 != name }
                return ProjectContext(
                    name: name,
                    source: .clientDeclared,
                    detail: detail.isEmpty ? nil : detail.joined(separator: " · ")
                )
            }
        }

        return ProjectContext(name: unidentifiedProjectName, source: .missingWorkspaceMetadata)
    }

    /// 列表行摘要用的一行归因。未识别与 Codex 情形的文案保持历史口径不变。
    public static func projectAttribution(
        eventKind: String,
        metadata: CodexMetadata?,
        declared: ClientDeclaredMetadata? = nil,
        projectedName: String? = nil,
        projectedSource: String? = nil,
        attributionScope: String? = nil
    ) -> String? {
        guard let context = projectContext(
            eventKind: eventKind,
            metadata: metadata,
            declared: declared,
            projectedName: projectedName,
            projectedSource: projectedSource,
            attributionScope: attributionScope
        )
        else { return nil }
        switch context.source {
        case .missingWorkspaceMetadata:
            return "未识别项目 · 来源未记录"
        case .clientDeclared:
            return "项目: \(context.name)（客户端声明）"
        default:
            return "项目: " + context.name
        }
    }

    private static func nonEmpty(_ value: String?) -> String? {
        guard let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines),
              !trimmed.isEmpty else { return nil }
        return trimmed
    }

    private static func workspaceProjectName(path: String, workspace: CodexWorkspaceMetadata?) -> String {
        if let local = path.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last,
           !local.isEmpty {
            return String(local)
        }
        if let remote = workspace?.associatedRemoteURLs.values.first,
           let repository = remote
                .replacingOccurrences(of: ".git", with: "")
                .split(whereSeparator: { $0 == "/" || $0 == ":" })
                .last,
           !repository.isEmpty {
            return String(repository)
        }
        return "未命名工作区"
    }

    /// Compact list label, retaining authoritative evidence for subagent rows.
    public static func codexSummary(_ metadata: CodexMetadata?) -> String? {
        guard let metadata, !metadata.isEmpty else { return nil }
        if metadata.isSubagent || metadata.subagentKind != nil || metadata.subagentHeader != nil {
            let kind = metadata.subagentKind ?? metadata.subagentHeader ?? "subagent"
            return ["Codex · 子代理(\(kind))", metadata.agentName]
                .compactMap { $0 }
                .joined(separator: " · ")
        }
        return ["Codex · 主代理", metadata.agentName]
            .compactMap { $0 }
            .joined(separator: " · ")
    }

    public static func codexJSON(_ metadata: CodexMetadata?) -> String? {
        guard let metadata else { return nil }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return try? String(decoding: encoder.encode(metadata), as: UTF8.self)
    }
    /// 兼容旧 stats.json：早期引擎会把 Codex/OpenAI 的正常无工具请求误记为
    /// `unmatched_no_tools`。仅隐藏这个已知误报，其他真实引擎标记原样保留。
    public static func messageForDisplay(
        _ message: String?,
        clientKind: ClientKind?
    ) -> String? {
        guard let message, !message.isEmpty else { return nil }
        guard clientKind == .codex || clientKind == .openaiCompat else { return message }
        let segments = message
            .components(separatedBy: "; ")
            .filter { $0 != "unmatched_no_tools" }
        return segments.isEmpty ? nil : segments.joined(separator: "; ")
    }

    /// 客户端断开/取消导致的写回失败特征(EPIPE/ECONNRESET/ENOTCONN 等)。
    public static func isClientDisconnect(message: String) -> Bool {
        let lowered = message.lowercased()
        let markers = [
            "posixerrorcode(rawvalue: 32",
            "posixerrorcode(rawvalue: 54",
            "posixerrorcode(rawvalue: 57",
            "broken pipe",
            "connection reset",
            "socket is not connected",
        ]
        return markers.contains { lowered.contains($0) }
    }

    /// 事件消息的中文摘要。返回空串表示无需展示(调用侧显示 `-`)。
    ///
    /// 消息词表见 rust/specs/spec-engine.md §5.1:引擎只发机器可读 token(`"; "` 连接),
    /// 翻译集中在这里。词表变更必须同步 spec、`message_tokens` 常量、引擎测试
    /// `MESSAGE_TOKEN_PREFIXES` 与本文件测试清单——两边清单互钉,漂移即红。
    public static func friendlyMessage(
        kind: String,
        statusCode: Int,
        failover: Bool,
        message: String?,
        outcome: RuntimeEventOutcome? = nil,
        phase: RuntimeEventPhase? = nil,
        failureKind: RuntimeFailureKind? = nil,
        timeoutMS: Int? = nil,
        upstreamStatusCode: Int? = nil,
        clientKind: ClientKind? = nil,
        streamTrace: StreamTrace? = nil
    ) -> String {
        let effectiveStatusCode = statusCode == 0 ? (upstreamStatusCode ?? statusCode) : statusCode
        if outcome == .cancelled || failureKind == .clientCancelled {
            return "客户端断开/取消"
        }
        if outcome == nil, effectiveStatusCode == 499 {
            return phase == .completed
                ? "最终结果未上报（HTTP 499 仅供参考）"
                : "客户端断开/取消"
        }
        // The user-facing explanation follows the recorded lifecycle result.
        // Protocol tokens and free-form engine notes stay in the diagnostic
        // fields instead of replacing “请求成功” or “流式输出完成”.
        if outcome == .succeeded {
            return streamTrace == nil ? "请求成功" : "流式输出完成"
        }
        var parts: [String] = []
        var explainsFailure = false
        let structuredFailure = failureKind.map {
            failureSummary(
                $0,
                kind: kind,
                statusCode: effectiveStatusCode,
                timeoutMS: timeoutMS,
                upstreamStatusCode: upstreamStatusCode
            )
        }
        if let message = messageForDisplay(message, clientKind: clientKind), !message.isEmpty {
            for segment in message.components(separatedBy: "; ") {
                let mapped = mapSegment(segment, kind: kind)
                // 新事件由 failureKind 给出唯一失败摘要；message 里的旧错误 token 仅为
                // 向后兼容，避免“连接失败 · 连接失败”重复。IP/桥接/轮数等信息仍保留。
                if !mapped.text.isEmpty, structuredFailure == nil || !mapped.explainsFailure {
                    parts.append(mapped.text)
                }
                if mapped.explainsFailure {
                    explainsFailure = true
                }
            }
        }
        if let structuredFailure {
            parts.insert(structuredFailure, at: 0)
            explainsFailure = true
        }
        // 失败行没有任何错误解释时按状态码合成一段——「消息列全空」的治本兜底:
        // 引擎对可重试状态只记 pinned 等信息 token,状态本身在这里翻译。
        if effectiveStatusCode >= 400, !explainsFailure {
            parts.insert("上游返回 \(effectiveStatusCode)\(statusCodeHint(effectiveStatusCode))", at: 0)
        } else if outcome == .failed, !explainsFailure {
            parts.insert("请求最终失败（HTTP 已返回 \(effectiveStatusCode)）", at: 0)
        }
        if parts.isEmpty, outcome == nil, phase == .completed, effectiveStatusCode != 0 {
            return "最终结果未上报（HTTP 状态仅供参考）"
        }
        if parts.isEmpty {
            return failover ? "故障转移" : ""
        }
        return parts.joined(separator: " · ")
    }

    private static func failureSummary(
        _ failureKind: RuntimeFailureKind,
        kind: String,
        statusCode: Int,
        timeoutMS: Int?,
        upstreamStatusCode: Int?
    ) -> String {
        switch failureKind {
        case .responseTimeout:
            if let timeoutMS {
                return "首响应超时（有效阈值 \(durationSeconds(timeoutMS))）"
            }
            return "首响应超时（尚未收到上游响应头）"
        case .connectionFailed:
            return kind == "upstream" ? "连接失败（未收到响应头）" : "代理连接上游失败"
        case .invalidResponse:
            return "上游响应无法解析"
        case .upstreamHTTPStatus:
            return "上游返回 HTTP \(upstreamStatusCode ?? statusCode)\(statusCodeHint(upstreamStatusCode ?? statusCode))"
        case .streamIdleTimeout:
            if let timeoutMS {
                return "流中断（\(durationSeconds(timeoutMS)) 未收到新数据）"
            }
            return "流中断（吐字空闲超时）"
        case .streamInterrupted:
            return "流中断（上游传输异常）"
        case .upstreamResponseIncomplete:
            return "上游响应未完整完成"
        case .upstreamResponseFailed:
            return "上游以失败状态结束响应"
        case .endpointsExhausted:
            return "所有可用入口均已尝试失败"
        case .clientCancelled:
            return "客户端断开/取消"
        case .clientRequestRejected:
            return "客户端请求被代理拒绝"
        }
    }

    private static func durationSeconds(_ milliseconds: Int) -> String {
        if milliseconds % 1_000 == 0 {
            return "\(milliseconds / 1_000) 秒"
        }
        return String(format: "%.1f 秒", Double(milliseconds) / 1_000)
    }

    private struct MappedSegment {
        let text: String
        /// true = 该段已解释失败原因,不再叠加状态码合成段。
        let explainsFailure: Bool
    }

    private static func mapSegment(_ segment: String, kind: String) -> MappedSegment {
        func info(_ text: String) -> MappedSegment {
            MappedSegment(text: text, explainsFailure: false)
        }
        func failure(_ text: String) -> MappedSegment {
            MappedSegment(text: text, explainsFailure: true)
        }

        // —— 现行词表(rust/specs/spec-engine.md §5.1)——
        if segment.hasPrefix("pinned ") {
            return info("IP 直连 " + String(segment.dropFirst("pinned ".count)))
        }
        if segment.hasPrefix("passthrough ") {
            // This is a routing token, not an outcome. The final result label
            // is derived from outcome/streamTrace by friendlyMessage.
            return info("")
        }
        if segment.hasPrefix("bridge ") {
            return info(String(segment.dropFirst("bridge ".count)) + " 桥接")
        }
        if segment.hasPrefix("deferred_rounds ") {
            return info("上游重跑 " + String(segment.dropFirst("deferred_rounds ".count)) + " 轮")
        }
        if segment == "unmatched_no_tools" {
            return info("无工具请求(用途未识别)")
        }
        if segment == "timeout" {
            return failure("响应超时")
        }
        if segment.hasPrefix("connection failed:") {
            return failure("连接失败")
        }
        if segment.hasPrefix("invalid response:") {
            return failure("上游响应无法解析")
        }
        if segment.hasPrefix("stream interrupted:") {
            let reason = segment.dropFirst("stream interrupted:".count)
            return failure(reason.contains("timeout") ? "流中断(吐字超时)" : "流中断(上游断流)")
        }
        if segment.hasPrefix("client_disconnected") {
            return failure("客户端断开/取消")
        }
        if segment == "upstream_retryable_status" {
            return failure("上游持续返回可重试错误")
        }
        if segment == "all endpoints failed" {
            return failure("所有入口均不可用(被停用或守卫跳过)")
        }
        if segment == "inbound_auth_required" {
            return failure("入站认证失败")
        }
        if segment == "openai_tools_unsupported" {
            return failure("openai 协议入口不支持工具调用,已跳过")
        }
        if segment == "body is not JSON" || segment == "body is not an object" {
            return failure("请求体不是合法 JSON")
        }
        if segment.hasPrefix("inbound_convert_failed:") {
            return failure("OpenAI 兼容请求无法转换")
        }
        if segment.hasPrefix("no pool accepts model") {
            return failure("没有匹配该模型的池")
        }
        if segment.hasPrefix("pool not found:") {
            return failure("目标池不存在")
        }
        if segment.hasPrefix("no enabled endpoint in pool") {
            return failure("目标池无可用入口")
        }
        if segment.hasPrefix("feature rule not found:") {
            return failure("分流规则不存在")
        }

        // —— legacy:Swift 时代 stats.json 历史事件(引擎已不再产出,只读兼容)——
        if segment == "no_endpoint" {
            return failure("池内无可用入口")
        }
        if let status = retryableStatus(in: segment) {
            return failure("上游返回 \(status)\(legacyRetryableHint(status))")
        }
        if isClientDisconnect(message: segment) {
            return failure(kind == "client" ? "客户端断开/取消" : "连接中断(对端断开)")
        }
        if segment.contains("connectionFailed") {
            return failure("连接失败")
        }
        if segment.contains("invalidResponse") {
            return failure("上游响应无法解析")
        }
        if segment.hasPrefix("noEnabledEndpoint") {
            return failure("目标池中没有该模型的可用映射")
        }
        if segment.hasPrefix("noPoolForModel") {
            return failure("没有匹配该模型的池")
        }
        if segment.hasPrefix("poolNotFound") {
            return failure("目标池不存在")
        }

        // 未识别:原样透传并视作失败解释(不再叠加合成段,行为对齐旧版兜底)。
        return failure(segment)
    }

    /// 消息列的视觉分级。
    ///
    /// 「200 但换过入口 / 上游重跑 6 轮 / 首字节等了 40s」和「200 一次打通」在原来的 UI 里
    /// 都是同样的灰色小字，代价看不见。`.warning` 是那条「成功了，但过程不太对」的线。
    public enum MessageSeverity: Sendable {
        /// 无消息:一次打通,消息列显示 `-`。
        case none
        /// 中性补充信息(IP 直连、桥接、上游重跑 1-2 轮…)。
        case info
        /// 成功但有代价,或明确的失败。
        case warning
    }

    /// 上游重跑到几轮算值得警示:1-2 轮是常态(一次上游失败就恢复),3 轮以上说明上游确实在挣扎。
    public static let noisyDeferredRounds = 3

    public static func messageSeverity(
        statusCode: Int,
        failover: Bool,
        message: String?,
        ttfbMS: Int? = nil,
        inFlight: Bool = false,
        outcome: RuntimeEventOutcome? = nil,
        failureKind: RuntimeFailureKind? = nil,
        upstreamStatusCode: Int? = nil,
        clientKind: ClientKind? = nil
    ) -> MessageSeverity {
        let effectiveStatusCode = statusCode == 0 ? (upstreamStatusCode ?? statusCode) : statusCode
        // 失败行:状态列已经是红的,消息列跟上一档,别让原因埋在灰字里。
        // 499 是用户主动取消,不算异常。
        if !inFlight,
           outcome == .failed || (effectiveStatusCode >= 400 && effectiveStatusCode != 499)
               || (failureKind != nil && failureKind != .clientCancelled) {
            return .warning
        }
        if failover || isSlowTTFB(ttfbMS) {
            return .warning
        }
        if let message = messageForDisplay(message, clientKind: clientKind), !message.isEmpty {
            for segment in message.components(separatedBy: "; ") {
                if segment.hasPrefix("deferred_rounds "),
                   let rounds = Int(segment.dropFirst("deferred_rounds ".count)),
                   rounds >= noisyDeferredRounds {
                    return .warning
                }
                // 未识别的内部请求:指纹可能已随 CC 升级失配,值得看见。
                if segment == "unmatched_no_tools" {
                    return .warning
                }
            }
            return .info
        }
        return .none
    }

    /// 真实协议名。`nil` 不是“未知协议”：它表示旧事件或请求尚未完成路由定型。
    public static func protocolDisplay(_ format: ProviderProtocol?) -> String {
        switch format {
        case .anthropic:
            return "Anthropic Messages"
        case .openai:
            return "OpenAI Chat Completions"
        case .openaiResponses:
            return "OpenAI Responses"
        case nil:
            return "未记录（旧事件或路由未完成）"
        }
    }

    public static func routeModeDisplay(_ mode: RouteMode?) -> String {
        switch mode {
        case .native:
            return "原生适配"
        case .translated:
            return "协议转换"
        case nil:
            return "未记录（旧事件或路由未完成）"
        }
    }

    /// 事件摘要用的协议路径。只依据事件中的真实字段，不从入口配置或自由文本推断。
    public static func protocolPath(
        sourceFormat: ProviderProtocol?,
        targetFormat: ProviderProtocol?,
        routeMode: RouteMode?
    ) -> String {
        let source = protocolDisplay(sourceFormat)
        let target = protocolDisplay(targetFormat)
        let mode = routeModeDisplay(routeMode)
        if sourceFormat == nil, targetFormat == nil, routeMode == nil {
            return source
        }
        return "\(source) → \(target) · \(mode)"
    }

    /// 详情页的次级诊断区是否有内容。真实工具调用本身就是重要诊断事实，
    /// 即使请求成功且没有失败/流追踪，也必须让该区块可见。
    public static func hasFailureToolOrStreamDiagnostics(_ event: RuntimeEvent) -> Bool {
        event.isFailed
            || event.failureKind != nil
            || event.failureDetail?.isEmpty == false
            || event.toolCalls?.isEmpty == false
            || event.timeoutMS != nil
            || event.streamTrace != nil
            || event.message?.isEmpty == false
    }

    /// 兼容旧的单列状态文案。新请求事件界面会把最终结果、HTTP 状态和阶段分开显示；
    /// 这里仍保留给诊断/旧调用方使用，且 0 明确表示从未收到响应头。
    public static func statusDisplay(
        _ statusCode: Int,
        inFlight: Bool = false,
        outcome: RuntimeEventOutcome? = nil,
        failureKind: RuntimeFailureKind? = nil,
        upstreamStatusCode: Int? = nil
    ) -> String {
        if inFlight {
            return "streaming"
        }
        let effectiveStatusCode = statusCode == 0 ? (upstreamStatusCode ?? statusCode) : statusCode
        if outcome == .cancelled {
            return effectiveStatusCode == 499 ? "取消" : "\(effectiveStatusCode) · 已取消"
        }
        if outcome == nil, effectiveStatusCode == 499 {
            return "取消"
        }
        if effectiveStatusCode == 0 {
            switch outcome {
            case .failed:
                return "未收到响应头 · 失败"
            case .cancelled:
                return "未收到响应头 · 已取消"
            default:
                return "未收到响应头"
            }
        }
        if outcome == .failed, (200..<400).contains(effectiveStatusCode) {
            switch failureKind {
            case .upstreamResponseFailed:
                return "\(effectiveStatusCode) · 上游协议失败"
            case .upstreamResponseIncomplete:
                return "\(effectiveStatusCode) · 响应未完成"
            case .streamIdleTimeout, .streamInterrupted:
                return "\(effectiveStatusCode) · 流中断"
            default:
                return "\(effectiveStatusCode) · 最终失败"
            }
        }
        return "\(effectiveStatusCode)"
    }

    public static func outcomeDisplay(
        _ outcome: RuntimeEventOutcome?,
        statusCode: Int,
        inFlight: Bool = false,
        eventKind: String? = nil,
        upstreamStatusCode: Int? = nil,
        phase: RuntimeEventPhase? = nil
    ) -> String {
        if eventKind == "notify", outcome == nil {
            return "不适用（通知事件）"
        }
        if inFlight {
            return "传输中"
        }
        let effectiveStatusCode = statusCode == 0 ? (upstreamStatusCode ?? statusCode) : statusCode
        switch outcome {
        case .succeeded: return "成功"
        case .failed: return "失败"
        case .cancelled: return "已取消"
        case nil where effectiveStatusCode == 499:
            return phase == .completed ? "已取消（最终结果未上报）" : "已取消（旧事件推断）"
        case nil where (200..<400).contains(effectiveStatusCode):
            return phase == .completed ? "成功（最终结果未上报）" : "成功（旧事件推断）"
        case nil:
            return phase == .completed ? "失败（最终结果未上报）" : "失败（旧事件推断）"
        }
    }

    /// 从引擎信息 token 中提取本次请求的协议处理方式。
    /// 这是排障事实，不从入口配置反推，避免透传模式忽略入口协议时显示错误结论。
    public static func forwardingModeDisplay(_ message: String?) -> String? {
        let segments = message?
            .split(separator: ";")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) } ?? []
        if segments.contains("passthrough responses") {
            return "Responses 原生适配"
        }
        if segments.contains("passthrough chat") {
            return "Chat Completions 原生适配"
        }
        if segments.contains("passthrough completions") {
            return "Legacy Completions 原生适配"
        }
        if segments.contains("passthrough responses-compact") {
            return "Responses Compact 原生适配"
        }
        if segments.contains("passthrough images-generations") {
            return "Images Generations 原生适配"
        }
        if segments.contains("passthrough images-edits") {
            return "Images Edits 原生适配"
        }
        if segments.contains("passthrough alpha-search") {
            return "Codex Alpha Search 原生适配"
        }
        if segments.contains("passthrough claude-count-tokens") {
            return "Claude Token Count 原生适配"
        }
        if segments.contains("bridge openai-responses") {
            return "Responses ↔ Anthropic 桥接"
        }
        if segments.contains("bridge openai") {
            return "Chat Completions ↔ Anthropic 桥接"
        }
        return nil
    }

    public static func failureKindDisplay(_ kind: RuntimeFailureKind) -> String {
        switch kind {
        case .responseTimeout: "首响应超时"
        case .connectionFailed: "连接失败"
        case .invalidResponse: "响应无法解析"
        case .upstreamHTTPStatus: "上游 HTTP 错误"
        case .streamIdleTimeout: "流空闲超时"
        case .streamInterrupted: "流传输中断"
        case .upstreamResponseIncomplete: "上游响应未完整"
        case .upstreamResponseFailed: "上游响应失败"
        case .endpointsExhausted: "入口耗尽"
        case .clientCancelled: "客户端取消"
        case .clientRequestRejected: "客户端请求被拒绝"
        }
    }

    public static func failurePhaseDisplay(_ phase: RuntimeFailurePhase) -> String {
        switch phase {
        case .beforeResponse: "收到响应头前"
        case .responseHeaders: "收到响应头时"
        case .responseStream: "响应流传输中"
        }
    }

    /// 耗时文案:进行中按事件开始时间显示已持续秒数；旧调用缺开始时间时保留占位符。
    /// 完成事件 1 秒内显示毫秒，否则显示秒。
    public static func durationDisplay(
        _ durationMS: Int,
        inFlight: Bool = false,
        startedAt: Date? = nil,
        now: Date = Date()
    ) -> String {
        if inFlight {
            guard let startedAt else {
                return "streaming…"
            }
            let elapsed = max(0, now.timeIntervalSince(startedAt))
            return String(format: "%.1fs", elapsed)
        }
        return durationMS < 1_000 ? "\(durationMS)ms" : String(format: "%.1fs", Double(durationMS) / 1_000)
    }

    /// Token 数量统一使用 ASCII 千位逗号，便于统计卡片和排行表快速扫读。
    ///
    /// 这里不使用裸字符串插值：那会绕过 Swift 的数字分组格式化，导致同一页
    /// 的普通计数有分隔符、Token 数量却没有分隔符。
    public static func tokenCountDisplay(_ value: Int?) -> String {
        guard let value else { return "—" }
        return value.formatted(
            .number
                .grouping(.automatic)
                .locale(Locale(identifier: "en_US"))
        )
    }

    /// 缓存读取命中率使用引擎按协议归一化后的处理输入作为分母。
    ///
    /// OpenAI 的缓存读取是 input 的子集，Anthropic 的缓存读写与普通 input
    /// 独立；Rust runtime store 已在 `processedInputTokens` 中按请求完成去重/
    /// 累加。UI 不应再次把 cacheRead 加到 input 上，否则子集口径会被重复计算。
    /// 口径未知、分母或必需字段缺失时返回破折号，不伪造百分比。
    public static func cacheHitRateDisplay(
        cacheReadInputTokens: Int?,
        processedInputTokens: Int?,
        tokenAccountingSemantics: String?,
        inputPresence: Int? = nil,
        cacheReadPresence: Int? = nil
    ) -> String {
        let values = (tokenAccountingSemantics ?? "")
            .split { $0 == "," || $0 == "|" }
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
            .filter { !$0.isEmpty }
        let unique = Set(values)
        guard !unique.isEmpty,
              !unique.contains("unknown"),
              unique.isSubset(of: ["subset", "independent", "mixed"]),
              let cacheReadInputTokens,
              let processedInputTokens,
              processedInputTokens > 0,
              cacheReadInputTokens >= 0,
              inputPresence == nil || inputPresence! > 0,
              cacheReadPresence == nil || cacheReadPresence! > 0 else {
            return "—"
        }
        let ratio = min(100, max(0, Double(cacheReadInputTokens) / Double(processedInputTokens) * 100))
        return String(format: "%.1f%%", locale: Locale(identifier: "en_US_POSIX"), ratio)
    }

    /// 首字节慢的判定阈值(ms)。
    ///
    /// 经验值:正常中转站的响应头在数秒内到达，超过 15s 基本意味着上游在排队或卡住
    /// ——而不是「这次输出比较长」。只用于染色提示，不参与任何超时/失败判定。
    public static let slowTTFBThresholdMS = 15_000

    public static func isSlowTTFB(_ ttfbMS: Int?) -> Bool {
        guard let ttfbMS else {
            return false
        }
        return ttfbMS >= slowTTFBThresholdMS
    }

    /// 耗时列文案:`首字节 → 总时长`。
    ///
    /// 流式请求的总时长是「吐完最后一个字」，单看它分不清「上游卡住 80s」和
    /// 「正常长输出 80s」。有 TTFB 时拆成两段展示，旧事件(nil)回退到只显示总时长。
    public static func durationWithTTFB(
        ttfbMS: Int?,
        durationMS: Int,
        inFlight: Bool = false,
        startedAt: Date? = nil,
        now: Date = Date()
    ) -> String {
        let total = durationDisplay(durationMS, inFlight: inFlight, startedAt: startedAt, now: now)
        guard let ttfbMS else {
            return total
        }
        // Upstream in-flight rows are timestamped when their headers arrive, while
        // TTFB is measured from attempt start. Do not render an impossible `8.0s → 0ms`.
        if inFlight, let startedAt {
            let elapsedMS = max(0, now.timeIntervalSince(startedAt) * 1_000)
            if elapsedMS < Double(max(0, ttfbMS)) {
                return "\(durationDisplay(ttfbMS)) → streaming…"
            }
        }
        return "\(durationDisplay(ttfbMS)) → \(total)"
    }

    private static func retryableStatus(in message: String) -> Int? {
        guard message.hasPrefix("retryableStatus("), message.hasSuffix(")") else {
            return nil
        }
        return Int(message.dropFirst("retryableStatus(".count).dropLast())
    }

    /// 状态码合成段的括注(无错误解释时的兜底);未知码不加括注。
    private static func statusCodeHint(_ status: Int) -> String {
        switch status {
        case 400: "(请求无效)"
        case 401: "(认证失败)"
        case 402: "(余额不足)"
        case 403: "(拒绝访问)"
        case 404: "(不存在)"
        case 408: "(请求超时)"
        case 429: "(限流)"
        case 500: "(服务端错误)"
        case 502: "(网关错误)"
        case 503: "(暂不可用)"
        case 504: "(网关超时)"
        case 520...530: "(源站不可达)"
        default: ""
        }
    }

    /// legacy `retryableStatus(NNN)` 的括注:该串只出现在可重试尝试上,默认「可重试」。
    private static func legacyRetryableHint(_ status: Int) -> String {
        switch status {
        case 401:
            return "(认证失败)"
        case 403:
            return "(拒绝访问)"
        case 429:
            return "(限流)"
        case 500:
            return "(服务端错误)"
        case 502:
            return "(网关错误)"
        case 503:
            return "(暂不可用)"
        case 520...530:
            return "(源站不可达)"
        default:
            return "(可重试)"
        }
    }
}
