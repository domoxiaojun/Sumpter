import AppKit
import SumpterCore
import SwiftUI

/// 事件表/详情的共享展示逻辑。
/// 此前运行页、统计页、诊断页各自实现,已经漂移过一次(诊断页 kind 漏中文化、
/// 模型箭头一处 "→" 一处 "->"),收敛到这里统一维护。
enum RuntimeEventDisplay {
    private static let local24HourTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .current
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    private static let local24HourDateTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .current
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter
    }()

    static func time(_ date: Date) -> String {
        local24HourTime.string(from: date)
    }

    static func dateTime(_ date: Date) -> String {
        local24HourDateTime.string(from: date)
    }

    static func kind(_ kind: String) -> String {
        switch kind {
        case "client": "客户端"
        case "upstream": "上游"
        case "notify": "通知"
        default: kind
        }
    }

    static func purpose(_ event: RuntimeEvent) -> String {
        if let purpose = event.requestPurpose {
            return purpose.displayName
        }
        if event.kind == "notify" {
            return "不适用（通知事件）"
        }
        // nil = 旧 stats.json 事件,或规划/鉴权阶段就被拒(还没识别出用途)。
        return event.requestPurpose?.displayName ?? "未记录（旧事件或早期拒绝）"
    }

    static func toolCalls(_ event: RuntimeEvent) -> String? {
        guard let calls = event.toolCalls, !calls.isEmpty else { return nil }
        return calls.joined(separator: "、")
    }

    static func codex(_ event: RuntimeEvent) -> String? {
        RuntimeEventPresentation.codexSummary(event.codexMetadata)
    }

    static func codexIdentity(_ event: RuntimeEvent) -> String? {
        let metadata = event.codexMetadata
        let values = [
            metadata?.threadID.map { "thread=\($0)" },
            metadata?.parentThreadID.map { "parent=\($0)" },
            metadata?.turnID.map { "turn=\($0)" },
            metadata?.requestKind.map { "request_kind=\($0)" },
            event.codexThreadClass.map { "功能线程=\($0)" },
            event.attributionScope.map { "归因范围=\($0)" }
        ].compactMap { $0 }
        return values.isEmpty ? nil : values.joined(separator: " · ")
    }

    static func codexAgentRole(_ metadata: CodexMetadata) -> String {
        if metadata.isSubagent || metadata.subagentKind != nil || metadata.subagentHeader != nil
            || metadata.threadSource == "subagent" || metadata.threadSource == "memory_consolidation" {
            return "子代理" + (metadata.subagentKind.map { " · \($0)" } ?? "")
        }
        if metadata.agentName != nil || metadata.threadID != nil || metadata.turnID != nil {
            return "主代理"
        }
        return "代理身份未确定"
    }

    static func codexWorkspaceSummary(_ metadata: CodexMetadata) -> String {
        guard !metadata.workspaces.isEmpty else { return "未记录项目 / 工作区" }
        return metadata.workspaces.keys.sorted().map { path in
            let workspace = metadata.workspaces[path]
            let project = workspaceProjectName(path: path, workspace: workspace)
            let status: String
            switch workspace?.hasChanges {
            case .some(true): status = "有未提交改动"
            case .some(false): status = "工作区干净"
            case .none: status = "状态未记录"
            }
            let commit = workspace?.latestGitCommitHash.map { "提交 \(String($0.prefix(8)))" }
            return [project, status, commit].compactMap { $0 }.joined(separator: " · ")
        }.joined(separator: "；")
    }

    static func codexWorkspaceRemoteSummary(_ metadata: CodexMetadata) -> String? {
        let remotes = metadata.workspaces.values
            .flatMap { $0.associatedRemoteURLs.values }
            .filter { !$0.isEmpty }
        return remotes.isEmpty ? nil : remotes.joined(separator: "；")
    }

    private static func workspaceProjectName(path: String, workspace: CodexWorkspaceMetadata?) -> String {
        if let localName = path.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last,
           !localName.isEmpty {
            return String(localName)
        }
        if let remote = workspace?.associatedRemoteURLs.values.first,
           let last = remote
               .replacingOccurrences(of: ".git", with: "")
            .split(whereSeparator: { $0 == "/" || $0 == ":" })
            .last,
           !last.isEmpty {
            return String(last)
        }
        return "未命名工作区"
    }

    static func requestSummary(_ event: RuntimeEvent) -> String {
        let metadata = event.codexMetadata
        return [
            RuntimeEventPresentation.projectAttribution(
                eventKind: event.kind,
                metadata: metadata,
                declared: event.clientDeclared,
                projectedName: event.projectName,
                projectedSource: event.projectSource
            ),
            kind(event.kind), clientKind(event),
            metadata.map { "代理: \(codexAgentRole($0))" },
            purpose(event),
            toolCalls(event).map { "工具: \($0)" },
            metadata?.agentName.map { "路径: \($0)" },
            event.featureRuleID.map { "规则: \(featureRule($0))" },
        ].compactMap { $0 }.joined(separator: " · ")
    }

    static func streamTrace(_ event: RuntimeEvent) -> String? {
        guard let trace = event.streamTrace else { return nil }
        var parts: [String] = []
        if let count = trace.chunkCount { parts.append("chunks=\(count)") }
        if let bytes = trace.bytesReceived { parts.append("bytes=\(bytes)") }
        if let gap = trace.maxChunkGapMS { parts.append("最大间隔=\(RuntimeEventPresentation.durationDisplay(gap))") }
        if let last = trace.lastChunkAtMS { parts.append("最后 chunk=\(RuntimeEventPresentation.durationDisplay(last))") }
        if let reason = trace.stopReason { parts.append("停止=\(reason)") }
        if let usage = trace.usage {
            let tokens = [
                usage.inputTokens.map { "输入 \(RuntimeEventPresentation.tokenCountDisplay($0))" },
                usage.outputTokens.map { "输出 \(RuntimeEventPresentation.tokenCountDisplay($0))" },
                usage.cacheReadInputTokens.map { "缓存读 \(RuntimeEventPresentation.tokenCountDisplay($0))" },
                usage.cacheCreationInputTokens.map { "缓存写 \(RuntimeEventPresentation.tokenCountDisplay($0))" },
                usage.reasoningTokens.map { "推理 \(RuntimeEventPresentation.tokenCountDisplay($0))" }
            ].compactMap { $0 }
            if !tokens.isEmpty { parts.append("Token: " + tokens.joined(separator: ", ")) }
        }
        if let last = trace.lastChunkAtMS, !event.isInFlight {
            parts.append("结束前空闲=\(RuntimeEventPresentation.durationDisplay(max(0, event.durationMS - last)))")
        }
        parts.append(
            trace.terminalEvent.map { "终止=\($0)" }
                ?? (event.isInFlight ? "等待协议终止" : "未观察到终止")
        )
        return parts.joined(separator: " · ")
    }

    /// 事件阶段与 HTTP 状态是两个独立维度；0 只表示尚未收到响应头。
    static func phase(_ event: RuntimeEvent) -> String {
        switch event.phase {
        case .inFlight:
            return "进行中"
        case .completed:
            return "已完成"
        case nil where event.outcome != nil:
            // 新事件已经有最终结果时，缺 phase 只代表旧 sidecar/UI 的字段缺口，
            // 不能再把当前请求标成“旧事件推断”。
            return "已完成"
        case nil where event.kind == "notify":
            return "不适用（通知事件）"
        case nil:
            return "旧事件（阶段未记录）"
        }
    }

    static func httpStatus(_ event: RuntimeEvent) -> String {
        let statusCode = event.effectiveHTTPStatusCode
        return statusCode == 0 ? "未收到响应头（0）" : "HTTP \(statusCode)"
    }

    static func outcome(_ event: RuntimeEvent) -> String {
        return RuntimeEventPresentation.outcomeDisplay(
            event.outcome,
            statusCode: event.statusCode,
            inFlight: event.isInFlight,
            eventKind: event.kind,
            upstreamStatusCode: event.upstreamStatusCode,
            phase: event.phase
        )
    }

    static func statusDetail(_ event: RuntimeEvent) -> String {
        "\(httpStatus(event)) · \(phase(event))"
    }

    /// 入站客户端。显式 unknown 是 UA 无法识别；nil 则是未记录（旧事件或早期拒绝），
    /// 非 client 事件不应该被误称为未知客户端。
    static func clientKind(_ event: RuntimeEvent) -> String {
        if let clientKind = event.clientKind {
            return clientKind.displayName
        }
        if event.kind == "notify" {
            return "不适用（通知事件）"
        }
        return "未记录（旧事件或早期拒绝）"
    }

    /// 事件列表使用路由记录的真实模型名。
    static func displayedModelName(_ raw: String?) -> String? {
        guard let raw else { return nil }
        let cleaned = ModelName.clean(raw)
        guard !cleaned.isEmpty else { return nil }
        return cleaned
    }

    static func model(_ event: RuntimeEvent) -> String {
        let candidates = [event.clientModel, event.effectiveModel]
            .compactMap(displayedModelName)
        var distinct: [String] = []
        for candidate in candidates where !distinct.contains(candidate) {
            distinct.append(candidate)
        }
        if distinct.isEmpty, let fallback = displayedModelName(event.upstreamModel) {
            return fallback
        }
        return distinct.isEmpty ? "-" : distinct.joined(separator: " → ")
    }

    static func endpoint(_ event: RuntimeEvent) -> String {
        let name = event.endpointName?.trimmingCharacters(in: .whitespacesAndNewlines)
        let id = event.endpointID?.trimmingCharacters(in: .whitespacesAndNewlines)
        let identity: String
        switch (name?.isEmpty == false ? name : nil, id?.isEmpty == false ? id : nil) {
        case let (.some(name), .some(id)):
            identity = "\(name) (\(id))"
        case let (.some(name), nil):
            identity = name
        case let (nil, .some(id)):
            identity = id
        default:
            identity = "-"
        }
        if let host = event.upstreamHost, !host.isEmpty { return "\(identity) @ \(host)" }
        return identity
    }

    /// 命中的特征规则:内建规则显示可读名(如 WebSearch),自定义规则回退到 id 本身。
    static func featureRule(_ ruleID: String) -> String {
        if let name = BuiltInFeatureRules.canonicalRule(id: ruleID)?.name {
            return "\(name) (\(ruleID))"
        }
        return ruleID
    }

    static func friendlyMessage(_ event: RuntimeEvent) -> String {
        if event.isInFlight {
            return "流式输出中"
        }
        return RuntimeEventPresentation.friendlyMessage(
            kind: event.kind,
            statusCode: event.statusCode,
            failover: event.failover,
            message: event.message,
            outcome: event.outcome,
            phase: event.phase,
            failureKind: event.failureKind,
            timeoutMS: event.timeoutMS,
            upstreamStatusCode: event.upstreamStatusCode,
            clientKind: event.clientKind,
            streamTrace: event.streamTrace
        )
    }

    static func statusColor(_ event: RuntimeEvent) -> Color {
        if event.isInFlight {
            return .blue
        }
        if event.kind == "notify", event.outcome == nil {
            return .secondary
        }
        if event.isCancelled {
            return .secondary
        }
        return event.isFailed ? .red : .green
    }

    /// 耗时列颜色:首字节慢的行染橙 —— 这是「上游在排队」和「输出本来就长」的分界。
    static func durationColor(_ event: RuntimeEvent) -> Color {
        if RuntimeEventPresentation.isSlowTTFB(event.ttfbMS) {
            return .orange
        }
        return event.isInFlight ? .blue : .primary
    }

    /// 耗时列悬停说明:两段各是什么、client 与 upstream 的口径差别。
    static func durationHelp(_ event: RuntimeEvent) -> String {
        guard event.ttfbMS != nil else {
            return "总耗时(该事件无首字节记录)"
        }
        let scope = event.kind == "upstream"
            ? "本次上游尝试的响应头延迟"
            : "客户端等待首字节的总时长（含故障转移与上游重跑）"
        let slow = RuntimeEventPresentation.isSlowTTFB(event.ttfbMS)
            ? "\n首字节超过 \(RuntimeEventPresentation.slowTTFBThresholdMS / 1_000)s,上游可能在排队。"
            : ""
        return "首字节 → 总耗时\n首字节 = \(scope)。\(slow)"
    }

    /// 消息列颜色:成功但有代价的行(换过入口、上游重跑多轮、首字节慢)不该和普通信息
    /// 一样是灰色小字。
    static func messageColor(_ event: RuntimeEvent, friendly: String) -> Color {
        if friendly.isEmpty {
            return .secondary
        }
        switch RuntimeEventPresentation.messageSeverity(
            statusCode: event.statusCode,
            failover: event.failover,
            message: RuntimeEventPresentation.messageForDisplay(
                event.message,
                clientKind: event.clientKind
            ),
            ttfbMS: event.ttfbMS,
            inFlight: event.isInFlight,
            outcome: event.outcome,
            failureKind: event.failureKind,
            upstreamStatusCode: event.upstreamStatusCode,
            clientKind: event.clientKind
        ) {
        case .warning: return .orange
        case .info, .none: return .primary
        }
    }
}

/// 紧凑但不丢语义的结果摘要：HTTP、最终结果和生命周期阶段始终分行显示。
private struct RuntimeEventStatusSummary: View {
    let event: RuntimeEvent
    var compact = false

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(RuntimeEventDisplay.outcome(event))
                .font((compact ? Font.caption : Font.callout).monospacedDigit().weight(.semibold))
                .foregroundStyle(RuntimeEventDisplay.statusColor(event))
                .lineLimit(1)
            Text(RuntimeEventDisplay.statusDetail(event))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
    }
}

private struct RuntimeEventTraceExport: Encodable {
    let id: String
    let timestamp: Date
    let requestID: String?
    let sessionID: String?
    let kind: String
    let endpointID: String?
    let endpointName: String?
    let upstreamHost: String?
    let clientKind: ClientKind?
    let sourceFormat: ProviderProtocol?
    let targetFormat: ProviderProtocol?
    let routeMode: RouteMode?
    let clientModel: String?
    let effectiveModel: String?
    let upstreamModel: String?
    let statusCode: Int
    let durationMS: Int
    let failover: Bool
    let message: String?
    let toolCalls: [String]?
    let requestPurpose: RequestPurpose?
    let outcome: RuntimeEventOutcome?
    let phase: RuntimeEventPhase?
    let featureRuleID: String?
    let failureDetail: String?
    let ttfbMS: Int?
    let timeoutMS: Int?
    let failureKind: RuntimeFailureKind?
    let failurePhase: RuntimeFailurePhase?
    let upstreamStatusCode: Int?
    let upstreamRequestID: String?
    let streamTrace: StreamTrace?
    let codexMetadata: CodexMetadata?

    init(event: RuntimeEvent) {
        id = event.id
        timestamp = event.timestamp
        requestID = event.requestID
        sessionID = event.sessionID
        kind = event.kind
        endpointID = event.endpointID
        endpointName = event.endpointName
        upstreamHost = event.upstreamHost
        clientKind = event.clientKind
        sourceFormat = event.sourceFormat
        targetFormat = event.targetFormat
        routeMode = event.routeMode
        clientModel = event.clientModel
        effectiveModel = event.effectiveModel
        upstreamModel = event.upstreamModel
        statusCode = event.statusCode
        durationMS = event.durationMS
        failover = event.failover
        message = event.message
        toolCalls = event.toolCalls
        requestPurpose = event.requestPurpose
        outcome = event.outcome
        phase = event.phase
        featureRuleID = event.featureRuleID
        failureDetail = event.failureDetail
        ttfbMS = event.ttfbMS
        timeoutMS = event.timeoutMS
        failureKind = event.failureKind
        failurePhase = event.failurePhase
        upstreamStatusCode = event.upstreamStatusCode
        upstreamRequestID = event.upstreamRequestID
        streamTrace = event.streamTrace
        codexMetadata = event.codexMetadata
    }

    func prettyJSON() -> String? {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return try? String(decoding: encoder.encode(self), as: UTF8.self)
    }
}

/// Mirrors the native event table's min/ideal column sizing for the separate
/// live-request stack. The live rows cannot live inside `Table` because their
/// shared animated background must span an arbitrary number of in-flight rows,
/// so resolve the same four-column proportions against the measured width.
private struct RuntimeEventColumnWidths {
    let request: CGFloat
    let route: CGFloat
    let result: CGFloat
    let message: CGFloat

    static func resolve(availableWidth: CGFloat) -> RuntimeEventColumnWidths {
        let minimums: [CGFloat] = [150, 180, 150, 160]
        let ideals: [CGFloat] = [178, 230, 170, 260]
        let minimumTotal = minimums.reduce(0, +)
        let idealTotal = ideals.reduce(0, +)
        let available = max(1, availableWidth)

        let widths: [CGFloat]
        if available < minimumTotal {
            let scale = available / minimumTotal
            widths = minimums.map { $0 * scale }
        } else {
            let remainder = available - minimumTotal
            widths = zip(minimums, ideals).map { minimum, ideal in
                minimum + remainder * ideal / idealTotal
            }
        }
        return RuntimeEventColumnWidths(
            request: widths[0],
            route: widths[1],
            result: widths[2],
            message: widths[3]
        )
    }
}

/// 「最近事件」面板:筛选 + 表格 + 选中详情。当前只挂在运行页；统计页只保留错误样本下钻。
struct RecentEventsPanel: View {
    let events: [RuntimeEvent]
    /// 实时进行中的事件单独显示，不计入稳定历史页的 totalCount/pageSize。
    var liveEvents: [RuntimeEvent] = []
    var hint = "客户端请求和上游尝试通过请求 ID 关联；HTTP 状态与最终结果分开显示，200 后的协议失败不会被误报为成功。"
    var detailedEvent: RuntimeEvent?
    var hasMore = false
    /// v2 stable-page metadata. Kept optional so the run view can continue to
    /// use the legacy cursor endpoint with the same component.
    var currentPage: Int?
    var totalPages: Int?
    var totalCount: Int?
    var pageLoading = false
    var hasPreviousPage = false
    var onLoadFirstPage: (() -> Void)?
    var onLoadPreviousPage: (() -> Void)?
    var onLoadNextPage: (() -> Void)?
    var onLoadLastPage: (() -> Void)?
    /// Arbitrary page jump; the named callbacks above remain for compatibility
    /// with older callers and for the first/previous/next/last affordances.
    var onPageChange: ((Int) -> Void)?
    var pageSize: Int?
    var onPageSizeChange: ((Int) -> Void)?
    /// 运行页把类型筛选下推服务端；旧调用方不提供时仍保留本地筛选。
    var initialKindFilter: RuntimeEventKindFilter?
    var onKindFilterChange: ((RuntimeEventKindFilter) -> Void)?
    var requestChainEvents: [RuntimeEvent]?
    var requestChainLoading = false
    var onSelectEvent: ((String?) -> Void)?
    var onLoadMore: (() -> Void)?
    @State private var selectedEventID: String?
    @Environment(\.sumpterPalette) private var palette
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    // 默认只看客户端:一次请求一行。排 failover 时切「全部」看完整上游尝试链。
    @State private var eventKindFilter: RuntimeEventKindFilter = .client

    /// Light surfaces use a darker, cooler spectrum so the rounded progress
    /// layer remains visible over the opaque panel. Opacity is controlled by
    /// the shared Canvas, keeping both themes on the same visual scale.
    private var liveAuraColors: [Color] {
        if colorScheme == .light {
            return [
                Color(red: 0.20, green: 0.50, blue: 0.70),
                Color(red: 0.47, green: 0.36, blue: 0.70),
                Color(red: 0.18, green: 0.58, blue: 0.54),
                Color(red: 0.76, green: 0.46, blue: 0.20),
            ]
        }
        return [palette.brand, palette.info, .purple, palette.warning]
    }

    private var eventRowHeight: CGFloat {
        dynamicTypeSize.isAccessibilitySize ? 72 : 48
    }

    var body: some View {
        SectionPanel(title: "最近事件", hint: hint) {
            VStack(alignment: .leading, spacing: 10) {
                eventToolbar
                if !visibleLiveEvents.isEmpty {
                    // Attach the aura as a background of the sized request
                    // stack.  An unconstrained ZStack/NSViewRepresentable has
                    // no intrinsic height in a SwiftUI VStack, so the old
                    // version could be laid out at zero height even though
                    // its Core Animation layers were running.
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(visibleLiveEvents.enumerated()), id: \.element.id) { index, event in
                            Button {
                                selectedEventID = event.id
                            } label: {
                                ViewThatFits(in: .horizontal) {
                                    liveEventRow(event)
                                        .frame(minWidth: 720)
                                    compactEventRow(event)
                                }
                            }
                            .buttonStyle(.plain)
                            .background(selectedEventID == event.id ? palette.brand.opacity(0.08) : .clear)
                            .accessibilityLabel("进行中请求：\(RuntimeEventDisplay.requestSummary(event))")
                            if index < visibleLiveEvents.count - 1 {
                                Divider().opacity(0.28)
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background {
                        // Pass the live group's exact size to the Canvas.  Keeping
                        // the sizing bridge at the container level avoids a
                        // transient zero-height proposal during Table updates.
                        GeometryReader { proxy in
                            RuntimeLiveBreathingAura(colors: liveAuraColors)
                                .frame(width: proxy.size.width, height: proxy.size.height)
                                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                        }
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityLabel("\(visibleLiveEvents.count) 个进行中请求")
                }
                if visibleEvents.isEmpty && visibleLiveEvents.isEmpty {
                    if pageLoading, currentPage != nil {
                        eventsTableContainer
                    } else {
                        EmptyStateView(
                            title: events.isEmpty ? "暂无事件" : "当前筛选下没有事件",
                            systemImage: "clock"
                        )
                    }
                } else if !visibleEvents.isEmpty {
                    eventsTableContainer
                }
                if let event = selectedEvent {
                    Divider()
                    ViewThatFits(in: .horizontal) {
                        HStack(alignment: .top, spacing: 16) {
                            RuntimeRequestTraceView(
                                chain: selectedRequestChain,
                                selectedEventID: $selectedEventID
                            )
                            .frame(width: 310, alignment: .topLeading)
                            Divider()
                            RuntimeEventDetail(event: event)
                                .frame(minWidth: 380, maxWidth: .infinity, alignment: .topLeading)
                        }
                        VStack(alignment: .leading, spacing: 16) {
                            RuntimeRequestTraceView(
                                chain: selectedRequestChain,
                                selectedEventID: $selectedEventID
                            )
                            Divider()
                            RuntimeEventDetail(event: event)
                                .frame(maxWidth: .infinity, alignment: .topLeading)
                        }
                    }
                }
                // Pagination belongs below the table: it follows the content
                // being paged and remains easy to reach without competing
                // with the filter at the top of the panel.
                eventPaginationFooter
            }
        }
        .onAppear { ensureSelection() }
        .onChange(of: visibleEventIDs) { _, _ in ensureSelection() }
        .onChange(of: selectedEventID) { _, eventID in onSelectEvent?(eventID) }
        .onChange(of: eventKindFilter) { _, filter in
            onKindFilterChange?(filter)
        }
        .onChange(of: currentPage) { _, _ in
            selectedEventID = nil
        }
        .onChange(of: pageSize) { _, _ in
            selectedEventID = nil
        }
        .onChange(of: initialKindFilter) { _, filter in
            guard let filter, filter != eventKindFilter else { return }
            eventKindFilter = filter
        }
    }

    private var eventsTableContainer: some View {
        ZStack(alignment: .topTrailing) {
            // `Table` is backed by NSTableView on macOS.  It does not reliably
            // mount row hosts while participating in SwiftUI transitions:
            // after a page refresh the header can remain while every row is
            // clipped away.  Keep the table's identity reset, but let the
            // native view appear directly in its fixed viewport.
            ViewThatFits(in: .horizontal) {
                eventsTable
                compactEventsList
            }
            .id(pageIdentity)
            .opacity(pageLoading ? 0.66 : 1)

            if pageLoading {
                HStack(spacing: 6) {
                    ProgressView()
                        .controlSize(.small)
                    Text("正在加载历史页…")
                        .font(.caption)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(palette.raised.opacity(0.94), in: Capsule())
                .overlay(Capsule().stroke(palette.borderSubtle, lineWidth: 0.8))
                .padding(10)
                .transition(.opacity)
                .allowsHitTesting(false)
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: eventTableHeight)
    }

    private var pageIdentity: String {
        "\(currentPage ?? 0)-\(pageSize ?? 0)-\(eventKindFilter.rawValue)"
    }

    /// The top toolbar is reserved for filtering.  Paging is rendered below
    /// the table so the content hierarchy reads filter → rows → navigation.
    @ViewBuilder
    private var eventToolbar: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) {
                eventKindPicker
                    .frame(width: 240)
                Spacer(minLength: 0)
            }

            VStack(alignment: .leading, spacing: 8) {
                eventKindPicker
                    .frame(maxWidth: .infinity)
            }
        }
    }

    private var eventKindPicker: some View {
        Picker("类型", selection: $eventKindFilter) {
            ForEach(RuntimeEventKindFilter.allCases, id: \.self) { filter in
                Text(filter.title).tag(filter)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(minHeight: 40)
    }

    private var eventCountLabel: some View {
        Text(totalCount.map { "本页 \(visibleEvents.count) 条 · 共 \($0) 条" } ?? "本页 \(visibleEvents.count) 条 · 共 \(events.count) 条")
            .font(.callout.monospacedDigit())
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .fixedSize(horizontal: true, vertical: false)
    }

    @ViewBuilder
    private var eventPaginationFooter: some View {
        if currentPage != nil, totalPages != nil {
            VStack(alignment: .leading, spacing: 8) {
                Divider()
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .center, spacing: 14) {
                        eventCountLabel
                        if requestChainLoading {
                            ProgressView().controlSize(.small)
                            Text("正在读取请求链…")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 8)
                        eventPageControls(compact: false)
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        HStack(spacing: 12) {
                            eventCountLabel
                            if requestChainLoading {
                                ProgressView().controlSize(.small)
                                Text("正在读取请求链…")
                                    .font(.callout)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        eventPageControls(compact: true)
                    }
                }
                Text("稳定历史快照；进行中事件单独显示，不占分页名额。")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        } else if hasMore {
            VStack(alignment: .leading, spacing: 8) {
                Divider()
                eventPageControls(compact: false)
            }
        }
    }

    @ViewBuilder
    private func eventPageControls(compact: Bool) -> some View {
        if let currentPage, let totalPages {
            SumpterPaginationControls(
                page: currentPage,
                totalPages: totalPages,
                hasPrevious: hasPreviousPage,
                hasNext: hasMore,
                pageSize: pageSize ?? AdminWire.RuntimeHistoryPage.defaultPageSize,
                compact: compact,
                loading: pageLoading,
                onPageChange: { target in
                    requestPage(target, currentPage: currentPage, totalPages: totalPages)
                },
                onPageSizeChange: { size in
                    guard !pageLoading else { return }
                    onPageSizeChange?(size)
                }
            )
        } else if hasMore, let onLoadMore {
            Button("加载更早一批（最多 1000 条）") {
                guard !pageLoading else { return }
                onLoadMore()
            }
            .disabled(pageLoading)
        }
    }

    private func requestPage(_ target: Int, currentPage: Int, totalPages: Int) {
        guard !pageLoading else { return }
        let clampedTarget = min(max(1, target), max(1, totalPages))
        guard clampedTarget != currentPage else { return }

        if clampedTarget == 1, let onLoadFirstPage {
            onLoadFirstPage()
        } else if clampedTarget == currentPage - 1, let onLoadPreviousPage {
            onLoadPreviousPage()
        } else if clampedTarget == currentPage + 1, let onLoadNextPage {
            onLoadNextPage()
        } else if clampedTarget == totalPages, let onLoadLastPage {
            onLoadLastPage()
        } else {
            onPageChange?(clampedTarget)
        }
    }

    /// The native history table and the animated live stack are separate
    /// containers, but their rows use these exact same cells. This keeps font,
    /// size, line height, truncation and semantic colour in sync.
    private func eventRequestCell(_ event: RuntimeEvent) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(RuntimeEventDisplay.time(event.timestamp))
                .font(.callout.monospacedDigit())
            Text(RuntimeEventDisplay.requestSummary(event))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .help(RuntimeEventDisplay.requestSummary(event))
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventRouteCell(_ event: RuntimeEvent) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(RuntimeEventDisplay.model(event))
                .font(.callout)
                .lineLimit(1)
                .help(RuntimeEventDisplay.model(event))
            Text(RuntimeEventDisplay.endpoint(event))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .help(RuntimeEventDisplay.endpoint(event))
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventResultCell(_ event: RuntimeEvent) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            RuntimeEventStatusSummary(event: event)
            RuntimeEventDurationText(event: event)
                .font(.caption.monospacedDigit())
                .foregroundStyle(RuntimeEventDisplay.durationColor(event))
                .help(RuntimeEventDisplay.durationHelp(event))
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventMessageCell(_ event: RuntimeEvent) -> some View {
        let friendly = RuntimeEventDisplay.friendlyMessage(event)
        return Text(friendly.isEmpty ? "-" : friendly)
            .foregroundStyle(RuntimeEventDisplay.messageColor(event, friendly: friendly))
            .font(.callout)
            .lineLimit(2)
            .help(
                event.failureDetail
                    ?? RuntimeEventPresentation.messageForDisplay(
                        event.message,
                        clientKind: event.clientKind
                    )
                    ?? friendly
            )
            .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func liveEventRow(_ event: RuntimeEvent) -> some View {
        GeometryReader { proxy in
            let outerInset: CGFloat = 8
            let widths = RuntimeEventColumnWidths.resolve(
                availableWidth: proxy.size.width - outerInset * 2
            )
            HStack(alignment: .center, spacing: 0) {
                eventRequestCell(event)
                    .padding(.horizontal, 12)
                    .frame(width: widths.request, alignment: .leading)
                eventRouteCell(event)
                    .padding(.horizontal, 12)
                    .frame(width: widths.route, alignment: .leading)
                eventResultCell(event)
                    .padding(.horizontal, 12)
                    .frame(width: widths.result, alignment: .leading)
                eventMessageCell(event)
                    .padding(.horizontal, 12)
                    .frame(width: widths.message, alignment: .leading)
            }
            .padding(.horizontal, outerInset)
            .frame(width: proxy.size.width, height: proxy.size.height, alignment: .leading)
        }
        .frame(height: eventRowHeight)
        .contentShape(Rectangle())
    }

    /// Narrow windows use a readable vertical summary instead of forcing the
    /// four desktop columns into a horizontal scroll region.  The same row
    /// remains selectable and opens the full detail view, so no capability is
    /// lost at the compact breakpoint.
    private func compactEventRow(_ event: RuntimeEvent) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(RuntimeEventDisplay.time(event.timestamp))
                    .font(.callout.monospacedDigit())
                RuntimeEventStatusSummary(event: event, compact: true)
                Spacer(minLength: 4)
                RuntimeEventDurationText(event: event)
                    .font(.caption.monospacedDigit())
            }
            Text("\(RuntimeEventDisplay.model(event)) · \(RuntimeEventDisplay.endpoint(event))")
                .font(.callout)
                .lineLimit(2)
                .foregroundStyle(palette.textPrimary)
            let friendly = RuntimeEventDisplay.friendlyMessage(event)
            if !friendly.isEmpty {
                Text(friendly)
                    .font(.caption)
                    .foregroundStyle(RuntimeEventDisplay.messageColor(event, friendly: friendly))
                    .lineLimit(dynamicTypeSize.isAccessibilitySize ? 3 : 2)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, dynamicTypeSize.isAccessibilitySize ? 10 : 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selectedEventID == event.id ? palette.brand.opacity(0.08) : .clear)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel("事件：\(RuntimeEventDisplay.requestSummary(event))")
    }

    private var compactEventsList: some View {
        ScrollView(.vertical, showsIndicators: true) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(visibleEvents) { event in
                    Button {
                        selectedEventID = event.id
                    } label: {
                        compactEventRow(event)
                    }
                    .buttonStyle(.plain)
                    if event.id != visibleEvents.last?.id {
                        Divider().opacity(0.35)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxWidth: .infinity)
        .frame(height: eventTableHeight)
        .background(palette.inset)
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        }
    }

    private var eventsTable: some View {
        Table(visibleEvents, selection: $selectedEventID) {
            TableColumn("请求") { event in
                eventRequestCell(event)
            }
            .width(min: 150, ideal: 178)
            TableColumn("模型 / 路由") { event in
                eventRouteCell(event)
            }
            .width(min: 180, ideal: 230)
            TableColumn("结果") { event in
                eventResultCell(event)
            }
            .width(min: 150, ideal: 170)
            TableColumn("说明") { event in
                eventMessageCell(event)
            }
            .width(min: 160, ideal: 260)
        }
        .sumpterTableSurface()
        .frame(minWidth: 920)
        .frame(height: eventTableHeight)
    }

    /// 事件表当前可见的行:按类型筛选 + 按时间倒序(引擎侧原地更新不移动行位置,排序在这里做)。
    private var visibleEvents: [RuntimeEvent] {
        RuntimeEvent.ordered(events, filter: eventKindFilter)
    }

    private var visibleLiveEvents: [RuntimeEvent] {
        RuntimeEvent.ordered(liveEvents, filter: eventKindFilter)
    }

    private var selectedEvent: RuntimeEvent? {
        guard let selectedEventID else {
            return nil
        }
        if detailedEvent?.id == selectedEventID {
            return detailedEvent
        }
        return (events + liveEvents).first { $0.id == selectedEventID }
    }

    private var selectedRequestChain: [RuntimeEvent] {
        guard let selectedEventID else { return [] }
        if let requestChainEvents, !requestChainEvents.isEmpty {
            return requestChainEvents
        }
        var chain = RuntimeEvent.requestChain(events + liveEvents, selectedID: selectedEventID)
        if let detailedEvent,
           let index = chain.firstIndex(where: { $0.id == detailedEvent.id }) {
            chain[index] = detailedEvent
        }
        return chain
    }

    private var visibleEventIDs: [String] { (visibleEvents + visibleLiveEvents).map(\.id) }

    private var eventTableHeight: CGFloat {
        // Reserve the same amount of space for adjacent history pages.  A
        // short final page should not pull the details below it upward while
        // the user is paging, and a loading response can keep the old table
        // in place until the replacement arrives.
        let visibleCount = visibleEvents.count
        let reservedCount = min(8, max(visibleCount, pageSize ?? visibleCount))
        return Swift.min(420, Swift.max(180, 34 + CGFloat(reservedCount) * eventRowHeight))
    }

    private func ensureSelection() {
        guard !visibleEventIDs.isEmpty else {
            selectedEventID = nil
            return
        }
        // 请求链允许在“仅客户端”筛选下点选隐藏的上游尝试。SSE 新事件到达会改变
        // visibleEventIDs；只要当前详情事件仍存在，就不能把选择重置回客户端行。
        if let selectedEventID, (events + liveEvents).contains(where: { $0.id == selectedEventID }) {
            return
        }
        // 首次进入运行页不自动打开第一条详情；详情只在用户明确点选
        // 一行后展开，避免实时刷新把用户的注意力强行带到新事件。
        selectedEventID = nil
    }
}

/// 同一 requestID 的完整上游尝试链。默认筛选只看客户端行时也能直接看到重试/failover。
private struct RuntimeRequestTraceView: View {
    let chain: [RuntimeEvent]
    @Binding var selectedEventID: String?

    private var clientEvent: RuntimeEvent? { chain.first { $0.kind == "client" } }
    private var upstreamAttempts: [RuntimeEvent] {
        Array(chain.filter { $0.kind == "upstream" }.reversed())
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text("请求链")
                    .font(.callout.weight(.semibold))
                Spacer()
                Text("\(upstreamAttempts.count) 次上游尝试")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            if let clientEvent {
                Button {
                    selectedEventID = clientEvent.id
                } label: {
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(spacing: 8) {
                            Label(RuntimeEventDisplay.clientKind(clientEvent), systemImage: "desktopcomputer")
                                .font(.caption.weight(.medium))
                            Spacer()
                        }
                        RuntimeEventStatusSummary(event: clientEvent, compact: true)
                        Text(RuntimeEventDisplay.model(clientEvent))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        Text([
                            clientEvent.requestID.map { "请求=\($0)" },
                            RuntimeEventDisplay.endpoint(clientEvent),
                            clientEvent.upstreamRequestID.map { "上游请求=\($0)" },
                            RuntimeEventDisplay.toolCalls(clientEvent).map { "工具=\($0)" },
                            RuntimeEventDisplay.codexIdentity(clientEvent)
                        ].compactMap { $0 }.joined(separator: " · "))
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("查看客户端请求详情")
            }
            Divider()
            if upstreamAttempts.isEmpty {
                Label("尚未产生上游尝试", systemImage: "clock")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(upstreamAttempts.indices, id: \.self) { index in
                    let attempt = upstreamAttempts[index]
                    attemptRow(attempt, number: index + 1)
                    if index < upstreamAttempts.count - 1 {
                        Divider().padding(.leading, 32)
                    }
                }
            }
        }
    }

    private func attemptRow(_ event: RuntimeEvent, number: Int) -> some View {
        let friendly = RuntimeEventDisplay.friendlyMessage(event)
        return Button {
            selectedEventID = event.id
        } label: {
            HStack(alignment: .top, spacing: 10) {
                Text("\(number)")
                    .font(.caption2.monospacedDigit().weight(.bold))
                    .foregroundStyle(RuntimeEventDisplay.statusColor(event))
                    .frame(width: 22, height: 22)
                    .background(RuntimeEventDisplay.statusColor(event).opacity(0.12), in: Circle())
                VStack(alignment: .leading, spacing: 3) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(RuntimeEventDisplay.endpoint(event))
                            .font(.callout.weight(event.id == selectedEventID ? .semibold : .medium))
                            .fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 6)
                    }
                    RuntimeEventStatusSummary(event: event, compact: true)
                    Text([
                        RuntimeEventDisplay.model(event),
                        RuntimeEventPresentation.durationWithTTFB(
                            ttfbMS: event.ttfbMS,
                            durationMS: event.durationMS,
                            inFlight: event.isInFlight,
                            startedAt: event.timestamp,
                            now: Date()
                        ),
                        event.endpointID.map { "入口 ID=\($0)" },
                        event.upstreamRequestID.map { "上游请求=\($0)" },
                        RuntimeEventDisplay.toolCalls(event).map { "工具=\($0)" }
                    ].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    if !friendly.isEmpty {
                        Text(friendly)
                            .font(.caption)
                            .foregroundStyle(RuntimeEventDisplay.messageColor(event, friendly: friendly))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let detail = event.failureDetail, !detail.isEmpty {
                        Text(detail)
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(.vertical, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("查看第 \(number) 次上游尝试详情")
    }
}

/// 选中事件按“操作摘要 → 诊断字段”分层。首屏只放判断请求是否正常所需的信息，
/// 长 ID、流计时与完整 Codex 元数据渐进披露，避免把所有采集字段铺成信息墙。
private struct RuntimeEventDetail: View {
    let event: RuntimeEvent
    @State private var copyFeedback: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text("选中事件")
                    .font(.callout.weight(.semibold))
                Spacer(minLength: 8)
                RuntimeEventStatusSummary(event: event, compact: true)
            }

            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                InfoRow(title: "时间", value: RuntimeEventDisplay.dateTime(event.timestamp))
                InfoRow(
                    title: "事件状态",
                    value: "\(RuntimeEventDisplay.kind(event.kind)) · \(RuntimeEventDisplay.phase(event))"
                )
                InfoRow(title: "最终结果", value: RuntimeEventDisplay.outcome(event))
                InfoRow(title: "HTTP 状态", value: RuntimeEventDisplay.httpStatus(event))
                runtimeDurationRow
                InfoRow(title: "入站客户端", value: RuntimeEventDisplay.clientKind(event))
                InfoRow(title: "请求用途", value: RuntimeEventDisplay.purpose(event))
                InfoRow(title: "模型链", value: modelChain)
                InfoRow(title: "路由入口", value: routeSummary)
                InfoRow(title: "故障转移", value: event.failover ? "是（已切换入口）" : "否")
                // 归因对 Codex 的结构化 workspace 和 Claude Code 的 X-Sumpter-* 声明都要生效;
                // 只看 codexMetadata 会让所有 CC 请求恒显示「未识别项目」。
                if let project = RuntimeEventPresentation.projectContext(
                    eventKind: event.kind,
                    metadata: event.codexMetadata,
                    declared: event.clientDeclared,
                    projectedName: event.projectName,
                    projectedSource: event.projectSource,
                    attributionScope: event.attributionScope
                ) {
                    let unidentified = project.source == .missingWorkspaceMetadata
                    let workspaceValue: String = if let metadata = event.codexMetadata,
                        !metadata.workspaces.isEmpty {
                        // Codex 情形交给 workspace 摘要:它还带干净/未提交与提交短哈希。
                        RuntimeEventDisplay.codexWorkspaceSummary(metadata)
                    } else if let detail = project.detail {
                        "\(project.name)（\(detail)）"
                    } else {
                        project.name
                    }
                    InfoRow(
                        title: "项目 / 工作区",
                        value: unidentified ? "未识别项目 · 来源未记录" : workspaceValue,
                        copyable: !unidentified,
                        muted: unidentified
                    )
                    InfoRow(title: "项目来源", value: project.source.label, muted: unidentified)
                }
                if let thread = event.codexThreadClass {
                    InfoRow(title: "功能线程", value: thread)
                }
                if let scope = event.attributionScope {
                    InfoRow(title: "归因范围", value: scope == "internal_feature" ? "后台功能" : scope)
                }
                if let declared = event.clientDeclared {
                    if let sourceProject = declared.sourceProject, !sourceProject.isEmpty {
                        InfoRow(title: "源项目", value: sourceProject, copyable: true)
                    }
                    if let sourceWorkspace = declared.sourceWorkspace, !sourceWorkspace.isEmpty {
                        InfoRow(title: "源工作区", value: sourceWorkspace, copyable: true)
                    }
                }
                if let metadata = event.codexMetadata {
                    InfoRow(title: "代理身份", value: RuntimeEventDisplay.codexAgentRole(metadata))
                    InfoRow(title: "代理路径", value: metadata.agentName ?? "未记录代理路径", copyable: metadata.agentName != nil)
                    if let remote = RuntimeEventDisplay.codexWorkspaceRemoteSummary(metadata) {
                        InfoRow(title: "远程仓库", value: remote, copyable: true)
                    }
                    if let installation = metadata.sourceInstallationID, !installation.isEmpty {
                        InfoRow(title: "源 installation", value: installation, copyable: true)
                    }
                    if !metadata.sourceWorkspacePaths.isEmpty {
                        InfoRow(
                            title: "源工作区路径",
                            value: metadata.sourceWorkspacePaths.joined(separator: "\n"),
                            copyable: true
                        )
                    }
                }
                if !friendly.isEmpty {
                    InfoRow(title: "摘要", value: friendly)
                } else if rawEngineMessage.isEmpty {
                    InfoRow(title: "摘要", value: "无(仅错误、IP 直连、桥接或上游重跑时记录)", muted: true)
                }
            }

            FullRowDisclosure(label: {
                Label("路由与请求标识", systemImage: "point.3.connected.trianglepath.dotted")
                    .font(.callout.weight(.semibold))
            }) {
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                    InfoRow(title: "事件 ID", value: event.id, copyable: true)
                    if let requestID = event.requestID, !requestID.isEmpty {
                        InfoRow(title: "请求 ID", value: requestID, copyable: true)
                    }
                    if let sessionID = event.sessionID, !sessionID.isEmpty {
                        InfoRow(title: "会话 ID", value: sessionID, copyable: true)
                    }
                    if let ruleID = event.featureRuleID, !ruleID.isEmpty {
                        InfoRow(title: "命中规则", value: RuntimeEventDisplay.featureRule(ruleID))
                    }
                    InfoRow(title: "客户端模型", value: RuntimeEventDisplay.displayedModelName(event.clientModel) ?? "-")
                    InfoRow(title: "路由模型", value: RuntimeEventDisplay.displayedModelName(event.effectiveModel) ?? "-")
                    InfoRow(title: "上游模型", value: RuntimeEventDisplay.displayedModelName(event.upstreamModel) ?? "-")
                    InfoRow(title: "入口名称", value: event.endpointName ?? "-")
                    InfoRow(title: "入口 ID", value: event.endpointID ?? "-", copyable: true)
                    InfoRow(title: "上游 Host", value: event.upstreamHost ?? "-", copyable: true)
                    InfoRow(title: "SourceFormat", value: RuntimeEventPresentation.protocolDisplay(event.sourceFormat))
                    InfoRow(title: "TargetFormat", value: RuntimeEventPresentation.protocolDisplay(event.targetFormat))
                    InfoRow(title: "路由模式", value: RuntimeEventPresentation.routeModeDisplay(event.routeMode))
                    InfoRow(
                        title: "协议路径",
                        value: RuntimeEventPresentation.protocolPath(
                            sourceFormat: event.sourceFormat,
                            targetFormat: event.targetFormat,
                            routeMode: event.routeMode
                        )
                    )
                    if let upstreamStatusCode = event.upstreamStatusCode {
                        InfoRow(title: "上游 HTTP 状态", value: "\(upstreamStatusCode)")
                    }
                    if let upstreamRequestID = event.upstreamRequestID, !upstreamRequestID.isEmpty {
                        InfoRow(title: "上游请求 ID", value: upstreamRequestID, copyable: true)
                    }
                    if let mode = RuntimeEventPresentation.forwardingModeDisplay(rawEngineMessage) {
                        InfoRow(title: "转发方式", value: mode)
                    }
                }
                .padding(.top, 8)
            }

            if hasFailureOrStreamDiagnostics {
                FullRowDisclosure(label: {
                    Label("失败、工具与流诊断", systemImage: "waveform.path.ecg")
                        .font(.callout.weight(.semibold))
                }) {
                    Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                        if let failureKind = event.failureKind {
                            InfoRow(title: "失败类型 / 阶段", value: failureSummary(failureKind))
                        } else if event.isFailed {
                            InfoRow(title: "失败类型 / 阶段", value: "未记录（旧事件）", muted: true)
                        }
                        if let toolCalls = RuntimeEventDisplay.toolCalls(event) {
                            InfoRow(title: "实际工具调用", value: toolCalls)
                        }
                        if let timeoutMS = event.timeoutMS {
                            InfoRow(title: "实际超时阈值", value: RuntimeEventPresentation.durationDisplay(timeoutMS))
                        }
                        streamRows
                        if let failureDetail = event.failureDetail, !failureDetail.isEmpty {
                            InfoRow(title: "技术详情", value: failureDetail, copyable: true)
                        }
                        if !rawEngineMessage.isEmpty {
                            InfoRow(title: "原始引擎消息", value: rawEngineMessage, copyable: true)
                        }
                    }
                    .padding(.top, 8)
                }
            }

            if let codex = event.codexMetadata {
                codexDisclosure(codex)
            }

            exportButtons
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    private var friendly: String { RuntimeEventDisplay.friendlyMessage(event) }
    private var rawEngineMessage: String { event.message ?? "" }

    private var modelChain: String {
        let rows: [(String, String?)] = [
            ("客户端", RuntimeEventDisplay.displayedModelName(event.clientModel)),
            ("路由", RuntimeEventDisplay.displayedModelName(event.effectiveModel)),
            ("上游", RuntimeEventDisplay.displayedModelName(event.upstreamModel))
        ]
        let values = rows.compactMap { row in row.1.map { "\(row.0)=\($0)" } }
        return values.isEmpty ? "未记录" : values.joined(separator: " → ")
    }

    private var routeSummary: String {
        RuntimeEventDisplay.endpoint(event)
    }

    private func failureSummary(_ kind: RuntimeFailureKind) -> String {
        RuntimeEventPresentation.failureKindDisplay(kind)
            + (event.failurePhase.map { " / " + RuntimeEventPresentation.failurePhaseDisplay($0) } ?? "")
    }

    private var hasFailureOrStreamDiagnostics: Bool {
        RuntimeEventPresentation.hasFailureToolOrStreamDiagnostics(event)
    }

    @ViewBuilder
    private var streamRows: some View {
        if let trace = event.streamTrace {
            InfoRow(
                title: "终止事件",
                value: trace.terminalEvent
                    ?? (event.isInFlight ? "等待协议终止" : "未观察到终止")
            )
            if let stopReason = trace.stopReason, !stopReason.isEmpty {
                InfoRow(title: "停止原因", value: stopReason)
            }
            if let usage = trace.usage {
                let usageParts: [String] = [
                    usage.inputTokens.map { value in "输入 (\(RuntimeEventPresentation.tokenCountDisplay(value)))" },
                    usage.outputTokens.map { value in "输出 (\(RuntimeEventPresentation.tokenCountDisplay(value)))" },
                    usage.cacheReadInputTokens.map { value in "缓存读 (\(RuntimeEventPresentation.tokenCountDisplay(value)))" },
                    usage.cacheCreationInputTokens.map { value in "缓存写 (\(RuntimeEventPresentation.tokenCountDisplay(value)))" },
                    usage.reasoningTokens.map { value in "推理 (\(RuntimeEventPresentation.tokenCountDisplay(value)))" }
                ].compactMap { value in value }
                if !usageParts.isEmpty {
                    InfoRow(title: "Token usage", value: usageParts.joined(separator: " · "))
                }
            }
            if let chunkCount = trace.chunkCount {
                InfoRow(title: "Chunk 数", value: "\(chunkCount)")
            }
            if let bytesReceived = trace.bytesReceived {
                InfoRow(title: "接收字节", value: ByteCountFormatter.string(fromByteCount: Int64(bytesReceived), countStyle: .binary))
            }
            if let maxChunkGapMS = trace.maxChunkGapMS {
                InfoRow(title: "最大 Chunk 间隔", value: RuntimeEventPresentation.durationDisplay(maxChunkGapMS))
            }
            if let lastChunkAtMS = trace.lastChunkAtMS {
                InfoRow(title: "最后 Chunk", value: RuntimeEventPresentation.durationDisplay(lastChunkAtMS))
                if !event.isInFlight {
                    InfoRow(
                        title: "结束前空闲",
                        value: RuntimeEventPresentation.durationDisplay(max(0, event.durationMS - lastChunkAtMS))
                    )
                }
            }
        }
    }

    private func codexDisclosure(_ metadata: CodexMetadata) -> some View {
        FullRowDisclosure(label: {
            Label("Codex 请求上下文", systemImage: "shippingbox")
                .font(.callout.weight(.semibold))
        }) {
            VStack(alignment: .leading, spacing: 10) {
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                    InfoRow(title: "代理身份", value: RuntimeEventDisplay.codexAgentRole(metadata))
                    InfoRow(title: "请求类型", value: metadata.requestKind ?? "未记录")
                    InfoRow(
                        title: "子代理类型",
                        value: metadata.subagentKind ?? (metadata.isSubagent ? "已标记，类型未记录" : "不适用（主代理）")
                    )
                    InfoRow(title: "线程来源", value: metadata.threadSource ?? "未记录")
                    InfoRow(title: "代理路径", value: metadata.agentName ?? "未记录", copyable: metadata.agentName != nil)
                    InfoRow(title: "项目 / 工作区", value: RuntimeEventDisplay.codexWorkspaceSummary(metadata), copyable: !metadata.workspaces.isEmpty)
                    if let remote = RuntimeEventDisplay.codexWorkspaceRemoteSummary(metadata) {
                        InfoRow(title: "远程仓库", value: remote, copyable: true)
                    }
                    InfoRow(
                        title: "工具命名空间",
                        value: metadata.toolNamespacesInfo.isEmpty
                            ? "未记录"
                            : metadata.toolNamespacesInfo.keys.sorted().joined(separator: "、"),
                        copyable: !metadata.toolNamespacesInfo.isEmpty
                    )
                    if let compaction = metadata.compaction {
                        InfoRow(title: "Compaction", value: compactionSummary(compaction))
                    }
                    InfoRow(title: "元数据状态", value: codexMetadataState(metadata))
                }
                FullRowDisclosure(label: {
                    Text("完整 Codex 技术字段")
                }) {
                    Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                        codexRows(metadata)
                    }
                    .padding(.top, 8)
                }
                .font(.caption.weight(.semibold))
            }
            .padding(.top, 8)
        }
    }

    private func compactionSummary(_ compaction: CodexCompactionMetadata) -> String {
        let parts = [
            compaction.trigger.map { "触发=\($0)" },
            compaction.reason.map { "原因=\($0)" },
            compaction.phase.map { "阶段=\($0)" },
            compaction.strategy.map { "策略=\($0)" }
        ].compactMap { $0 }
        return parts.isEmpty ? "已记录" : parts.joined(separator: " · ")
    }

    private func codexMetadataState(_ metadata: CodexMetadata) -> String {
        let states = [
            metadata.malformed ? "格式异常" : nil,
            metadata.truncated ? "已截断" : nil,
            metadata.hasConflicts ? "字段冲突" : nil,
            metadata.redactedFields.isEmpty ? nil : "含脱敏字段"
        ].compactMap { $0 }
        return states.isEmpty ? "正常" : states.joined(separator: " · ")
    }

    private var exportButtons: some View {
        VStack(alignment: .leading, spacing: 6) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { eventExportButton; codexExportButton }
                VStack(alignment: .leading, spacing: 8) { eventExportButton; codexExportButton }
            }
            if let copyFeedback {
                Label(copyFeedback, systemImage: copyFeedback == "已复制" ? "checkmark.circle" : "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(copyFeedback == "已复制" ? .green : .red)
                    .accessibilityAddTraits(.updatesFrequently)
            }
        }
    }

    @ViewBuilder
    private var eventExportButton: some View {
        if let payload = RuntimeEventTraceExport(event: event).prettyJSON() {
            Button {
                copyToPasteboard(payload)
            } label: {
                Label("复制源事件 JSON", systemImage: "doc.on.doc")
            }
            .buttonStyle(.bordered)
            .help("复制事件、路由、状态、失败、工具调用、流诊断与有界源归属元数据；不包含请求或响应正文与凭据")
        }
    }

    @ViewBuilder
    private var codexExportButton: some View {
        if let payload = RuntimeEventPresentation.codexJSON(event.codexMetadata) {
            Button {
                copyToPasteboard(payload)
            } label: {
                Label("复制完整 Codex 元数据 JSON", systemImage: "doc.on.doc")
            }
            .buttonStyle(.bordered)
            .help("复制所有已记录的 Codex 回合、代理、工作区和工具命名空间字段")
        }
    }

    private func copyToPasteboard(_ payload: String) {
        copyFeedback = PasteboardCopy.write(payload) ? "已复制" : "复制失败"
    }

    private var runtimeDurationRow: some View {
        GridRow {
            Text(event.ttfbMS == nil ? "总耗时" : "TTFB / 总耗时")
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(width: 92, alignment: .trailing)
            RuntimeEventDurationText(event: event)
                .font(.callout.monospacedDigit())
                .foregroundStyle(RuntimeEventDisplay.durationColor(event))
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .help(RuntimeEventDisplay.durationHelp(event))
        }
    }

    @ViewBuilder
    private func codexRows(_ metadata: CodexMetadata) -> some View {
        let rows: [(String, String?)] = [
            ("installation ID", metadata.installationID), ("session ID", metadata.sessionID),
            ("thread ID", metadata.threadID), ("agent path", metadata.agentName),
            ("turn ID", metadata.turnID),
            ("window ID", metadata.windowID), ("request kind", metadata.requestKind),
            ("forked-from thread ID", metadata.forkedFromThreadID),
            ("parent thread ID", metadata.parentThreadID), ("parent turn ID", metadata.parentTurnID),
            ("root turn ID", metadata.rootTurnID), ("x-openai-subagent", metadata.subagentHeader),
            ("subagent kind", metadata.subagentKind), ("thread source", metadata.threadSource),
            ("sandbox", metadata.sandbox), ("sandbox mode", metadata.sandboxMode),
            ("originator", metadata.originator),
            ("beta features", metadata.betaFeatures), ("memgen request", metadata.memgenRequest),
            ("responses lite", metadata.responsesLite),
            ("sources", metadata.sources.isEmpty ? nil : metadata.sources.joined(separator: ", ")),
            ("redacted fields", metadata.redactedFields.isEmpty ? nil : metadata.redactedFields.joined(separator: ", ")),
            ("conflicts", metadata.conflicts.isEmpty ? nil : metadata.conflicts.joined(separator: ", "))
        ]
        ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
            if let value = row.1, !value.isEmpty { InfoRow(title: row.0, value: value, copyable: true) }
        }
        if let value = metadata.turnStartedAtUnixMS { InfoRow(title: "turn started (Unix ms)", value: "\(value)") }
        if let value = metadata.wsStreamRequestStartMS { InfoRow(title: "WS request start (ms)", value: "\(value)") }
        InfoRow(title: "auto review", value: metadata.autoReviewEnabled.map { $0 ? "true" : "false" } ?? "-")
        InfoRow(title: "Node REPL review required", value: metadata.nodeReplAutoReviewRequired.map { $0 ? "true" : "false" } ?? "-")
        InfoRow(title: "Node REPL disabled", value: metadata.nodeReplDisabled.map { $0 ? "true" : "false" } ?? "-")
        InfoRow(title: "is subagent", value: metadata.isSubagent ? "true" : "false")
        InfoRow(title: "parent inferred", value: metadata.parentThreadIDInferred ? "true" : "false")
        InfoRow(title: "malformed", value: metadata.malformed ? "true" : "false")
        InfoRow(title: "truncated", value: metadata.truncated ? "true" : "false")
        InfoRow(title: "has conflicts", value: metadata.hasConflicts ? "true" : "false")
        if let compaction = metadata.compaction {
            let rows: [(String, String?)] = [
                ("trigger", compaction.trigger), ("reason", compaction.reason),
                ("implementation", compaction.implementation), ("phase", compaction.phase),
                ("strategy", compaction.strategy)
            ]
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                if let value = row.1, !value.isEmpty {
                    InfoRow(title: "compaction \(row.0)", value: value, copyable: true)
                }
            }
        }
        ForEach(metadata.workspaces.keys.sorted(), id: \.self) { path in
            if let workspace = metadata.workspaces[path] {
                InfoRow(title: "本地路径", value: path, copyable: true)
                if let commit = workspace.latestGitCommitHash, !commit.isEmpty {
                    InfoRow(title: "workspace commit", value: commit, copyable: true)
                }
                if let hasChanges = workspace.hasChanges {
                    InfoRow(title: "workspace has changes", value: hasChanges ? "true" : "false")
                }
                ForEach(workspace.associatedRemoteURLs.keys.sorted(), id: \.self) { remoteKey in
                    if let remoteURL = workspace.associatedRemoteURLs[remoteKey] {
                        InfoRow(title: "workspace remote \(remoteKey)", value: remoteURL, copyable: true)
                    }
                }
            }
        }
        ForEach(metadata.toolNamespacesInfo.keys.sorted(), id: \.self) { namespaceKey in
            if let namespace = metadata.toolNamespacesInfo[namespaceKey] {
                InfoRow(title: "tool namespace", value: namespace.name.map { "\(namespaceKey) (\($0))" } ?? namespaceKey, copyable: true)
                ForEach(namespace.functions.keys.sorted(), id: \.self) { functionKey in
                    if let function = namespace.functions[functionKey] {
                        let source = function.source.map {
                            [$0.kind, $0.serverName].compactMap { $0 }.joined(separator: ":")
                        }
                        let attributes = [
                            function.name.map { "name=\($0)" },
                            function.direct.map { "direct=\($0)" },
                            function.codeModeName.map { "codeModeName=\($0)" },
                            function.deferred.map { "deferred=\($0)" },
                            source.map { "source=\($0)" }
                        ].compactMap { $0 }.joined(separator: " · ")
                        InfoRow(
                            title: "tool function \(functionKey)",
                            value: attributes.isEmpty ? "已记录" : attributes,
                            copyable: true
                        )
                    }
                }
            }
        }
        ForEach(metadata.extras.keys.sorted(), id: \.self) { key in
            if let value = metadata.extras[key] {
                InfoRow(title: "extra \(key)", value: value, copyable: true)
            }
        }
        let state = [metadata.malformed ? "malformed" : nil, metadata.truncated ? "truncated" : nil, metadata.hasConflicts ? "有冲突" : nil].compactMap { $0 }.joined(separator: " · ")
        InfoRow(title: "metadata 状态", value: state.isEmpty ? "正常" : state)
    }
}

/// Provider 候选序列 → 模型规则/入口数摘要（运行页与路由页共用）。
struct PoolSummaryList: View {
    let config: AppConfig

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                summaryIcon
                summaryText
                Spacer(minLength: 8)
                providerCount
            }
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    summaryIcon
                    Text("Provider 候选序列")
                        .font(.callout.weight(.semibold))
                    Spacer(minLength: 8)
                    providerCount
                }
                Text(config.modelSummary())
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var summaryIcon: some View {
        Image(systemName: "arrow.triangle.2.circlepath")
            .foregroundStyle(.blue)
            .frame(width: 20)
            .accessibilityHidden(true)
    }

    private var summaryText: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("Provider 候选序列")
                .font(.callout.weight(.semibold))
            Text(config.modelSummary())
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(3)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var providerCount: some View {
        Text("\(config.endpoints.count) 个 Provider")
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: true, vertical: false)
    }
}
