import CryptoKit
import Foundation

public struct AnthropicMessage: Codable, Equatable, Sendable {
    public var role: String
    public var content: JSONValue

    public init(role: String, content: JSONValue) {
        self.role = role
        self.content = content
    }
}

public struct RoutingRequest: Codable, Equatable, Sendable {
    public var model: String
    public var system: JSONValue?
    public var messages: [AnthropicMessage]
    public var tools: [[String: JSONValue]]
    public var raw: [String: JSONValue]

    public init(
        model: String,
        system: JSONValue? = nil,
        messages: [AnthropicMessage] = [],
        tools: [[String: JSONValue]] = [],
        raw: [String: JSONValue] = [:]
    ) {
        self.model = model
        self.system = system
        self.messages = messages
        self.tools = tools
        self.raw = raw
    }
}

public enum RouteMode: String, Codable, Equatable, Sendable {
    case native
    case translated
}

public struct PlannedEndpoint: Equatable, Sendable, Identifiable {
    public var id: String { endpointID }
    public var endpointID: String
    public var endpointName: String
    public var baseURL: URL
    public var configuredProtocol: EndpointProtocolMode
    public var sourceFormat: ProviderProtocol
    public var providerProtocol: ProviderProtocol
    public var userAgent: UserAgentSettings
    public var upstreamModel: String
    /// 同组入口共享会话粘性;nil = 使用入口 id 作为独立组并参与统一 Provider 分流。
    public var stickyGroup: String?
    public var thinking: ThinkingMode
    public var context: ContextMode
    public var effortOverride: ReasoningEffort?
    public var failoverTimeoutSeconds: Double?
    public var priority: Int
    public var modelGroupID: String? = nil
    public var modelGroupRank: Int = 0
    public var schedulingStrategy: ModelGroupSchedulingStrategy = .priority

    public var routeMode: RouteMode {
        sourceFormat == providerProtocol ? .native : .translated
    }

    public init(
        endpointID: String,
        endpointName: String,
        baseURL: URL,
        configuredProtocol: EndpointProtocolMode = .anthropic,
        sourceFormat: ProviderProtocol = .anthropic,
        providerProtocol: ProviderProtocol,
        upstreamModel: String,
        userAgent: UserAgentSettings = UserAgentSettings(),
        stickyGroup: String? = nil,
        thinking: ThinkingMode,
        context: ContextMode,
        effortOverride: ReasoningEffort? = nil,
        failoverTimeoutSeconds: Double? = nil,
        priority: Int = 0
    ) {
        self.endpointID = endpointID
        self.endpointName = endpointName
        self.baseURL = baseURL
        self.configuredProtocol = configuredProtocol
        self.sourceFormat = sourceFormat
        self.providerProtocol = providerProtocol
        self.userAgent = userAgent
        self.upstreamModel = upstreamModel
        self.stickyGroup = stickyGroup
        self.thinking = thinking
        self.context = context
        self.effortOverride = effortOverride
        self.failoverTimeoutSeconds = failoverTimeoutSeconds
        self.priority = max(0, priority)
    }

    /// Legacy source compatibility only. Provider candidates are now flat and
    /// no pool identifier is persisted or emitted in the wire contract.
    @available(*, deprecated, message: "Provider 池已移除；请使用 endpointID")
    public init(
        poolID _: String,
        endpointID: String,
        endpointName: String,
        baseURL: URL,
        configuredProtocol: EndpointProtocolMode = .anthropic,
        sourceFormat: ProviderProtocol = .anthropic,
        providerProtocol: ProviderProtocol,
        upstreamModel: String,
        stickyGroup: String? = nil,
        thinking: ThinkingMode,
        context: ContextMode,
        effortOverride: ReasoningEffort? = nil,
        failoverTimeoutSeconds: Double? = nil,
        priority: Int = 0
    ) {
        self.init(
            endpointID: endpointID,
            endpointName: endpointName,
            baseURL: baseURL,
            configuredProtocol: configuredProtocol,
            sourceFormat: sourceFormat,
            providerProtocol: providerProtocol,
            upstreamModel: upstreamModel,
            stickyGroup: stickyGroup,
            thinking: thinking,
            context: context,
            effortOverride: effortOverride,
            failoverTimeoutSeconds: failoverTimeoutSeconds,
            priority: priority
        )
    }

    @available(*, deprecated, message: "Provider 池已移除；请使用 endpointID")
    public var poolID: String {
        get { "providers" }
        set { /* ignored */ }
    }

    /// 账号级调度的分组键:没设粘性分组的入口用自己的 id 作为独立组。
    public var schedulingGroup: String {
        stickyGroup ?? endpointID
    }
}

public struct RoutePlan: Equatable, Sendable {
    public var clientModel: String
    public var effectiveModel: String
    public var featureRuleID: String?
    public var endpoints: [PlannedEndpoint]

    @available(*, deprecated, message: "Provider 池已移除；请使用 endpoints")
    public var poolID: String {
        get { "providers" }
        set { /* ignored */ }
    }
}

public enum RoutePlanningError: Error, Equatable, Sendable {
    case noProviderForModel(String)
    case providerNotFound(String)
    case noEnabledProvider
    case noCompatibleProvider(provider: String, sourceFormat: ProviderProtocol)

    /// Legacy error cases kept only so older callers can display a useful
    /// message while upgrading. New planning code never constructs them.
    @available(*, deprecated, message: "Provider 池已移除；请使用 noProviderForModel/noCompatibleProvider")
    case noPoolForModel(String)
    @available(*, deprecated, message: "Provider 池已移除；请使用 providerNotFound")
    case poolNotFound(String)
    @available(*, deprecated, message: "Provider 池已移除；请使用 noEnabledProvider")
    case noEnabledEndpoint(String)
    @available(*, deprecated, message: "Provider 池已移除；请使用 noCompatibleProvider")
    case noCompatibleProtocol(pool: String, sourceFormat: ProviderProtocol)
}

/// 请求本身的用途，仅用于日志、统计和排障，不参与模型路由。
///
/// `standard` 表示没有命中已知的 Claude Code 独立子请求形状；不能仅凭模型别名
/// （例如用户把 Haiku/Sonnet 都映射到 gpt-5.6-luna）推断用途。
public enum RequestPurpose: String, Codable, CaseIterable, Equatable, Sendable {
    case standard
    case sessionTitle = "session_title"
    case webSearch = "websearch"
    case webFetch = "webfetch"
    case classifier
    case compact
    case imageGeneration = "image_generation"
    case imageEdit = "image_edit"
    case alphaSearch = "alpha_search"
    case tokenCount = "token_count"

    public var displayName: String {
        switch self {
        case .standard:
            return "主请求"
        case .sessionTitle:
            return "会话标题（Claude Code 内部）"
        case .webSearch:
            return "WebSearch"
        case .webFetch:
            return "WebFetch"
        case .classifier:
            return "自动模式分类器"
        case .compact:
            return "上下文压缩"
        case .imageGeneration:
            return "图片生成"
        case .imageEdit:
            return "图片编辑"
        case .alphaSearch:
            return "Codex 独立搜索"
        case .tokenCount:
            return "Token 计数"
        }
    }
}

public enum RequestInspector {
    private static let claudeCodeIdentity = "You are Claude Code, Anthropic's official CLI for Claude."
    private static let webSearchSystem = "You are an assistant for performing a web search tool use"
    private static let webSearchMessagePrefix = "Perform a web search for the query: "
    private static let webFetchPrefix = "Web page content:\n---\n"
    private static let webFetchConciseSuffix = "Provide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed."
    private static let webFetchRestrictedSuffix = "- Never produce or reproduce exact song lyrics."
    private static let classifierSystem = "You are a security monitor for autonomous AI coding agents."
    private static let sessionTitleSystemPrefix = "Write the title in "
    private static let sessionTitleSystemSuffix = "Keep technical terms and code identifiers in their original form."

    public static func systemText(_ request: RoutingRequest) -> String {
        text(fromSystem: request.system)
    }

    public static func firstUserText(_ request: RoutingRequest) -> String {
        guard let message = request.messages.first(where: { $0.role == "user" }) else {
            return ""
        }
        if let text = message.content.stringValue {
            return text
        }
        return message.content.pythonStyleJSONString()
    }

    public static func hasToolType(_ request: RoutingRequest, prefix: String) -> Bool {
        guard !prefix.isEmpty else {
            return false
        }
        var hasTarget = false
        var hasClientTool = false
        for tool in request.tools {
            let type = tool["type"]?.stringValue ?? ""
            if type.hasPrefix(prefix) {
                hasTarget = true
            } else if type.isEmpty {
                hasClientTool = true
            }
        }
        return hasTarget && !hasClientTool
    }

    public static func messagesContain(_ request: RoutingRequest, needle: String) -> Bool {
        guard !needle.isEmpty else {
            return false
        }
        let lowered = needle.lowercased()
        return request.messages.contains { message in
            text(fromContent: message.content, prefixLimit: 800).lowercased().contains(lowered)
        }
    }

    /// 严格识别 Claude Code 内建的三类独立请求。
    ///
    /// 三个检测器必须互斥：未命中或异常地同时命中多个时返回 nil，
    /// 由普通模型路由接管，不再依赖规则排列顺序。
    public static func detectedRequestKind(_ request: RoutingRequest) -> FeatureRequestKind? {
        let matches: [FeatureRequestKind] = [
            matchesWebSearch(request) ? .websearch : nil,
            matchesWebFetch(request) ? .webfetch : nil,
            matchesClassifier(request) ? .classifier : nil
        ].compactMap { $0 }
        return matches.count == 1 ? matches[0] : nil
    }

    /// 为运行日志和统计提供用途标签。用途识别与路由解耦，标题生成不会触发任何特征分流。
    public static func requestPurpose(_ request: RoutingRequest) -> RequestPurpose {
        let featureKind = detectedRequestKind(request)
        let isSessionTitle = matchesSessionTitle(request)

        // 异常请求同时长得像标题生成和内建分流时不猜测，按普通请求展示。
        if isSessionTitle {
            return featureKind == nil ? .sessionTitle : .standard
        }
        switch featureKind {
        case .websearch:
            return .webSearch
        case .webfetch:
            return .webFetch
        case .classifier:
            return .classifier
        case nil:
            return .standard
        }
    }

    public static func featureRule(_ rule: FeatureRule, matches request: RoutingRequest) -> Bool {
        guard rule.enabled else {
            return false
        }

        var conditionCount = 0
        if let requestKind = rule.match.requestKind {
            conditionCount += 1
            guard detectedRequestKind(request) == requestKind else {
                return false
            }
        }
        if let toolTypePrefix = rule.match.toolTypePrefix, !toolTypePrefix.isEmpty {
            conditionCount += 1
            guard hasToolType(request, prefix: toolTypePrefix) else {
                return false
            }
        }
        if let systemContains = rule.match.systemContains, !systemContains.isEmpty {
            conditionCount += 1
            guard systemText(request).localizedCaseInsensitiveContains(systemContains) else {
                return false
            }
        }
        if let messagesContain = rule.match.messagesContain, !messagesContain.isEmpty {
            conditionCount += 1
            guard Self.messagesContain(request, needle: messagesContain) else {
                return false
            }
        }
        if let modelEquals = rule.match.modelEquals, !modelEquals.isEmpty {
            conditionCount += 1
            guard ModelName.clean(request.model) == ModelName.clean(modelEquals) else {
                return false
            }
        }
        return conditionCount > 0
    }

    private static func matchesWebSearch(_ request: RoutingRequest) -> Bool {
        guard request.messages.count == 1,
              request.messages[0].role == "user",
              let message = strictText(from: request.messages[0].content),
              normalizedNewlines(message).trimmingCharacters(in: .whitespacesAndNewlines)
                .hasPrefix(webSearchMessagePrefix),
              systemText(request).contains(webSearchSystem),
              request.tools.count == 1,
              request.tools[0]["name"]?.stringValue == "web_search",
              request.tools[0]["type"]?.stringValue?.hasPrefix("web_search") == true,
              forcedToolChoice(request, name: "web_search") else {
            return false
        }
        return true
    }

    private static func matchesWebFetch(_ request: RoutingRequest) -> Bool {
        guard request.tools.isEmpty,
              request.messages.count == 1,
              request.messages[0].role == "user",
              systemText(request).contains(claudeCodeIdentity),
              let rawText = strictText(from: request.messages[0].content) else {
            return false
        }

        let text = normalizedNewlines(rawText).trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.hasPrefix(webFetchPrefix) else {
            return false
        }
        let contentAndTail = text.dropFirst(webFetchPrefix.count)
        guard contentAndTail.contains("\n---\n") else {
            return false
        }
        return text.hasSuffix(webFetchConciseSuffix)
            || (text.contains("Provide a concise response based only on the content above. In your response:")
                && text.hasSuffix(webFetchRestrictedSuffix))
    }

    private static func matchesClassifier(_ request: RoutingRequest) -> Bool {
        guard systemText(request).contains(classifierSystem),
              let lastMessage = request.messages.last,
              lastMessage.role == "user",
              let rawText = strictText(from: lastMessage.content) else {
            return false
        }

        let text = normalizedNewlines(rawText).trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.hasPrefix("<transcript>\n"),
              text.contains("\n</transcript>") else {
            return false
        }

        if request.tools.isEmpty {
            // 当前 XML fast/thinking 两阶段：stage 1 可带 stop_sequences，stage 2 不带。
            guard let stopSequences = request.raw["stop_sequences"] else {
                return true
            }
            guard case .array(let values) = stopSequences else {
                return false
            }
            return values.contains(.string("</block>")) || values.contains(.string("</severity>"))
        }

        // 兼容旧单阶段分类器：只暴露 classify_result 并强制调用。
        guard request.tools.count == 1,
              request.tools[0]["name"]?.stringValue == "classify_result" else {
            return false
        }
        return forcedToolChoice(request, name: "classify_result")
    }

    /// Claude Code 2.1.x 自动会话标题请求：专用 system、单条 `<session>` 消息、无工具。
    /// 不看模型名，避免把用户主动选择的 gpt-5.6-luna 主请求误标成内部辅助请求。
    private static func matchesSessionTitle(_ request: RoutingRequest) -> Bool {
        guard request.tools.isEmpty,
              request.messages.count == 1,
              request.messages[0].role == "user",
              let rawText = strictText(from: request.messages[0].content) else {
            return false
        }

        let system = normalizedNewlines(systemText(request))
        guard system.contains(sessionTitleSystemPrefix),
              system.contains(sessionTitleSystemSuffix) else {
            return false
        }
        let text = normalizedNewlines(rawText).trimmingCharacters(in: .whitespacesAndNewlines)
        return text.hasPrefix("<session>") && text.hasSuffix("</session>")
    }

    private static func forcedToolChoice(_ request: RoutingRequest, name: String) -> Bool {
        guard case .object(let choice) = request.raw["tool_choice"] else {
            return false
        }
        return choice["type"]?.stringValue == "tool" && choice["name"]?.stringValue == name
    }

    /// 严格请求指纹只接受纯字符串或纯 text block；tool_result 及嵌套 content
    /// 只供自定义 messagesContain 使用，不能再参与内建 WebFetch 识别。
    private static func strictText(from value: JSONValue) -> String? {
        switch value {
        case .string(let text):
            return text
        case .array(let blocks):
            guard !blocks.isEmpty else {
                return nil
            }
            var parts: [String] = []
            for block in blocks {
                guard case .object(let object) = block,
                      (object["type"]?.stringValue ?? "text") == "text",
                      let text = object["text"]?.stringValue else {
                    return nil
                }
                parts.append(text)
            }
            return parts.joined(separator: "\n")
        default:
            return nil
        }
    }

    private static func normalizedNewlines(_ value: String) -> String {
        value.replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
    }

    private static func text(fromSystem value: JSONValue?) -> String {
        guard let value else {
            return ""
        }
        switch value {
        case .string(let text):
            return text
        case .array(let blocks):
            return blocks.compactMap { block -> String? in
                if case .object(let object) = block {
                    return object["text"]?.stringValue
                }
                return nil
            }.joined(separator: "\n")
        default:
            return ""
        }
    }

    private static func text(fromContent value: JSONValue, prefixLimit: Int? = nil) -> String {
        let rendered: String
        switch value {
        case .string(let raw):
            rendered = raw
        case .array(let blocks):
            rendered = blocks.map { block in
                if case .object(let object) = block {
                    if let direct = object["text"]?.stringValue {
                        return direct
                    }
                    if let nested = object["content"] {
                        return text(fromContent: nested)
                    }
                }
                return ""
            }.filter { !$0.isEmpty }.joined(separator: " ")
        default:
            rendered = ""
        }
        if let prefixLimit {
            return String(rendered.prefix(prefixLimit))
        }
        return rendered
    }
}

public enum StickyHasher {
    public static func sessionKey(for request: RoutingRequest) -> String {
        let system = String(RequestInspector.systemText(request).prefix(4_000))
        let firstUser = String(RequestInspector.firstUserText(request).prefix(2_000))
        let data = Data("\(system)|\(firstUser)".utf8)
        let digest = Insecure.MD5.hash(data: data)
        return digest.map { String(format: "%02x", $0) }.joined()
    }

    public static func homeIndex(for request: RoutingRequest, candidateCount: Int) -> Int {
        guard candidateCount > 1 else {
            return 0
        }
        let key = sessionKey(for: request)
        let prefix = String(key.prefix(8))
        return Int(prefix, radix: 16).map { $0 % candidateCount } ?? 0
    }
}

public struct RoutePlanner {
    public init() {}

    /// `sourceFormat` 是入站路径已经确定的真实协议。旧调用方不传时按
    /// Anthropic（即历史 `/v1/messages` 路径）处理；不会从 UA 或 body 猜测。
    public func plan(
        request: RoutingRequest,
        config: AppConfig,
        sourceFormat: ProviderProtocol = .anthropic
    ) throws -> RoutePlan {
        let baseModel = ModelName.clean(request.model)
        let featureRule = config.featureRules.first { RequestInspector.featureRule($0, matches: request) }
        let target = featureRule?.target
        let effectiveModel = ModelName.clean(target?.model ?? baseModel)

        if let featureRule, let target {
            let pinnedEndpoint = target.endpointID.flatMap { endpointID in
                config.endpoints.first { $0.id == endpointID && $0.enabled }
            }
            var featureEndpoints = plannedEndpoints(
                endpoints: config.endpoints,
                effectiveModel: effectiveModel,
                sourceFormat: sourceFormat,
                protocolOverride: target.protocolOverride,
                pinnedEndpointID: target.endpointID,
                effortOverride: target.effortOverride
            )
            // 规则钉住的入口只有在被停用/删除时才降级为整池 failover。
            // 入口仍启用但固定协议不匹配时必须明确失败，不能偷偷改走别的入口。
            if featureEndpoints.isEmpty, target.endpointID != nil, pinnedEndpoint == nil {
                featureEndpoints = plannedEndpoints(
                    endpoints: config.endpoints,
                    effectiveModel: effectiveModel,
                    sourceFormat: sourceFormat,
                    protocolOverride: target.protocolOverride,
                    effortOverride: target.effortOverride
                )
            }
            guard !featureEndpoints.isEmpty else {
                throw RoutePlanningError.noCompatibleProvider(
                    provider: target.endpointID ?? "candidates",
                    sourceFormat: sourceFormat
                )
            }
            return RoutePlan(
                clientModel: baseModel,
                effectiveModel: effectiveModel,
                featureRuleID: featureRule.id,
                endpoints: featureEndpoints
            )
        }

        let scoped = config.routingEndpoints(for: baseModel)
        let matching = scoped.filter { $0.preferredMapping(for: baseModel) != nil }
        guard !matching.isEmpty else { throw RoutePlanningError.noProviderForModel(baseModel) }
        let endpoints = plannedEndpoints(
            endpoints: scoped,
            effectiveModel: baseModel,
            sourceFormat: sourceFormat
        )

        guard !endpoints.isEmpty else {
            throw RoutePlanningError.noCompatibleProvider(
                provider: "candidates",
                sourceFormat: sourceFormat
            )
        }

        return RoutePlan(
            clientModel: baseModel,
            effectiveModel: baseModel,
            featureRuleID: nil,
            endpoints: endpoints
        )
    }

    /// protocolOverride 非 nil 时是硬目标协议：Auto 可解析为该协议，固定入口必须匹配。
    /// 未指定时 Auto 解析成 SourceFormat，固定入口解析成自身协议。只要存在原生候选，
    /// 就不把桥接候选混进同一次请求的重试列表。
    /// pinnedEndpointID 非 nil 时(feature route 钉住了某个入口)只保留该入口,并跳过模型映射
    /// 筛选 —— 显式点名即强制走它。
    private func plannedEndpoints(
        endpoints: [Endpoint],
        effectiveModel: String,
        sourceFormat: ProviderProtocol,
        protocolOverride: ProviderProtocol? = nil,
        pinnedEndpointID: String? = nil,
        effortOverride: ReasoningEffort? = nil
    ) -> [PlannedEndpoint] {
        let candidates = endpoints
            .filter(\.enabled)
            .filter { pinnedEndpointID == nil || $0.id == pinnedEndpointID }
            .compactMap { endpoint -> PlannedEndpoint? in
                // 每个入口都通过显式 mappings 声明承接范围。
                let mapping = endpoint.preferredMapping(for: effectiveModel)
                // 规则钉住的入口跳过这层筛选:显式点名即强制走它。
                if pinnedEndpointID == nil, mapping == nil {
                    return nil
                }
                let failoverTimeout = mapping?.failoverTimeoutSeconds
                guard let providerProtocol = endpoint.protocolMode.resolve(
                    sourceFormat: sourceFormat,
                    targetOverride: protocolOverride
                ) else {
                    // 固定协议入口不能被规则改成另一个 TargetFormat；Auto 才可按规则覆盖。
                    return nil
                }
                let upstream = mapping?.upstreamModel(for: effectiveModel) ?? effectiveModel
                var planned = PlannedEndpoint(
                    endpointID: endpoint.id,
                    endpointName: endpoint.name,
                    baseURL: endpoint.baseURL,
                    configuredProtocol: endpoint.protocolMode,
                    sourceFormat: sourceFormat,
                    providerProtocol: providerProtocol,
                    upstreamModel: upstream,
                    userAgent: endpoint.userAgent,
                    stickyGroup: endpoint.stickyGroup,
                    thinking: mapping?.thinking ?? .adaptive,
                    context: mapping?.context ?? .oneMillion,
                    effortOverride: effortOverride ?? mapping?.effort,
                    failoverTimeoutSeconds: failoverTimeout,
                    priority: endpoint.priority
                )
                planned.modelGroupID = endpoint.modelGroupID
                planned.modelGroupRank = endpoint.modelGroupRank
                planned.schedulingStrategy = endpoint.modelGroupSchedulingStrategy
                return planned
            }
        let native = candidates.filter { $0.routeMode == .native }
        return orderEndpoints(native.isEmpty ? candidates : native)
    }

    /// 无既有会话归属时的确定性入口顺序：按调度组最低 priority，
    /// 同级按该组首次出现位置，组内保持配置顺序。
    private func orderEndpoints(_ endpoints: [PlannedEndpoint]) -> [PlannedEndpoint] {
        var groups: [(id: String, rank: Int, priority: Int, firstIndex: Int)] = []
        for (index, endpoint) in endpoints.enumerated() {
            if let groupIndex = groups.firstIndex(where: { $0.id == endpoint.schedulingGroup }) {
                groups[groupIndex].priority = min(groups[groupIndex].priority, endpoint.priority)
            } else {
                groups.append((endpoint.schedulingGroup, endpoint.modelGroupRank, endpoint.priority, index))
            }
        }
        groups.sort {
            if $0.rank != $1.rank { return $0.rank < $1.rank }
            if $0.priority != $1.priority { return $0.priority < $1.priority }
            return $0.firstIndex < $1.firstIndex
        }
        return groups.flatMap { group in
            endpoints.filter { $0.schedulingGroup == group.id }
        }
    }
}
