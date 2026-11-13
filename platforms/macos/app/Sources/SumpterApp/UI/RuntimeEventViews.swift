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
        formatter.timeZone = .autoupdatingCurrent
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    private static let local24HourMonthDayTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .autoupdatingCurrent
        formatter.dateFormat = "MM-dd HH:mm:ss"
        return formatter
    }()

    private static let local24HourDateTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = .autoupdatingCurrent
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter
    }()

    static func time(_ date: Date, includeDate: Bool = true) -> String {
        if !includeDate || Calendar.current.isDateInToday(date) {
            return local24HourTime.string(from: date)
        }
        return local24HourMonthDayTime.string(from: date)
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

    /// 列表行只显示已记录的用途（主对话 / WebSearch / 自动模式分类器等），
    /// 让分流请求在列表里就能被认出来；未记录时不占位，缺失原因留给详情面板解释。
    static func rowPurpose(_ event: RuntimeEvent) -> String? {
        if let kind = event.requestKind { return RuntimeEventPresentation.observedRequestKind(kind) }
        return event.requestPurpose?.displayName
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
        RuntimeEventPresentation.codexRole(metadata)
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
        [RuntimeEventPresentation.projectAttribution(
            eventKind: event.kind, metadata: event.codexMetadata, declared: event.clientDeclared,
            projectedName: event.projectName, projectedSource: event.projectSource, projectedLocalUser: event.localUser
        ), kind(event.kind), clientKind(event), event.agentSummaryLabel, rowPurpose(event), event.sourceIP].compactMap { $0 }.joined(separator: " · ")
    }

    static func streamTrace(_ event: RuntimeEvent) -> String? {
        guard let trace = event.streamTrace else { return nil }
        if let websocket = trace.websocketTrace { return RuntimeEventPresentation.websocketSummary(websocket) }
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

    static func httpStatusColor(_ event: RuntimeEvent) -> Color {
        switch event.statusCode {
        case 101: .accentColor
        case 200..<300: .green
        case 300..<400: .orange
        case 400...: .red
        default: .secondary
        }
    }

    static func httpStatus(_ event: RuntimeEvent) -> String {
        let statusCode = event.effectiveHTTPStatusCode
        return statusCode == 0 ? "未收到响应头（0）" : "HTTP \(statusCode)"
    }

    static func outcome(_ event: RuntimeEvent) -> String {
        if event.isCancelled && event.failureKind == .clientDisconnected { return "连接中断" }
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

    static func logicalModel(_ event: RuntimeEvent) -> String {
        displayedModelName(event.effectiveModel) ?? "—"
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
        for value in [event.endpointName, event.endpointID] {
            if let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty {
                return value
            }
        }
        return "-"
    }

    /// 命中的特征规则:内建规则显示可读名(如 WebSearch),自定义规则回退到 id 本身。
    static func featureRule(_ ruleID: String) -> String {
        if let name = BuiltInFeatureRules.canonicalRule(id: ruleID)?.name {
            return "\(name) (\(ruleID))"
        }
        return ruleID
    }

    static func friendlyMessage(_ event: RuntimeEvent) -> String {
        if event.isInFlight, event.streamTrace?.websocketTrace == nil {
            if event.effectiveHTTPStatusCode == 0 { return "等待响应" }
            return event.streamTrace == nil ? "接收响应中" : "流式输出中"
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
        if RuntimeEventPresentation.isSlowTTFB(displayTiming(event).ttfbMS) {
            return .orange
        }
        return event.isInFlight ? .blue : .primary
    }

    /// 已完成客户端请求的列表时间轴与输出速度保持一致，使用最终上游尝试。
    /// 进行中请求仍使用客户端时间轴，保证总耗时实时增长。
    static func displayTiming(_ event: RuntimeEvent) -> (ttfbMS: Int?, durationMS: Int) {
        if event.kind == "client", !event.isInFlight,
           let ttfb = event.upstreamTTFBMS, let duration = event.upstreamDurationMS {
            return (ttfb, duration)
        }
        return (event.ttfbMS, event.durationMS)
    }

    /// 耗时列悬停说明:两段各是什么、client 与 upstream 的口径差别。
    static func durationHelp(_ event: RuntimeEvent) -> String {
        let timing = displayTiming(event)
        guard timing.ttfbMS != nil else {
            return "总耗时(该事件无首字节记录)"
        }
        let scope = event.kind == "client" && !event.isInFlight
            ? "最终上游尝试的响应头延迟"
            : event.kind == "upstream" ? "本次上游尝试的响应头延迟" : "客户端等待首字节的总时长（含故障转移与上游重跑）"
        let slow = RuntimeEventPresentation.isSlowTTFB(timing.ttfbMS)
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

/// 结果 pill。进行中的那一枚是整行唯一的活动提示:macOS 26 上用蓝 tint 的 interactive 玻璃,
/// 加一个点做 phase 呼吸;更早系统退回实色底。历史行永远是实色底,滚动时不走玻璃折射。
private struct RuntimeEventStatusPill: View {
    let text: String
    let color: Color
    var live = false
    var compact = false

    var body: some View {
        HStack(spacing: 5) {
            if live {
                RuntimeLiveBreathingDot(color: color)
            }
            Text(text)
                .font((compact ? Font.caption : Font.subheadline).monospacedDigit().weight(.semibold))
                .lineLimit(1)
        }
        .foregroundStyle(color)
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .fixedSize(horizontal: true, vertical: false)
        .modifier(RuntimeEventStatusPillSurface(color: color, live: live))
    }
}

private struct RuntimeEventStatusPillSurface: ViewModifier {
    let color: Color
    let live: Bool

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: 5, style: .continuous)
        if live, #available(macOS 26.0, *) {
            content.glassEffect(.regular.tint(color.opacity(0.22)).interactive(), in: shape)
        } else {
            content.background(color.opacity(live ? 0.12 : 0.08), in: shape)
        }
    }
}

/// 结果只强调最终成败；HTTP 响应头作为独立的次级信息。
private struct RuntimeEventStatusSummary: View {
    let event: RuntimeEvent
    var compact = false
    var includeUsage = true

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            RuntimeEventStatusPill(
                text: event.isInFlight ? "进行中" : RuntimeEventDisplay.outcome(event),
                color: RuntimeEventDisplay.statusColor(event),
                live: event.isInFlight,
                compact: compact
            )
            Text(event.effectiveHTTPStatusCode == 0 ? "HTTP —" : RuntimeEventDisplay.httpStatus(event))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
                .help(RuntimeEventDisplay.statusDetail(event))
            if includeUsage && event.kind != "notify" {
                RuntimeEventUsageSummary(event: event)
            }
        }
    }
}

private struct RuntimeEventUsageSummary: View {
    let event: RuntimeEvent

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            if event.isInFlight && !event.hasObservedUsage {
                // 进行中且上游还没报任何用量：只放一个占位，不用两行文字解释“还没有”。
                Text("—")
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(.tertiary)
            } else {
                Text(event.usageSummaryLabel)
                    .font(.subheadline.monospacedDigit().weight(.medium))
                    .foregroundStyle(event.hasObservedUsage ? Color.primary : Color.secondary)
                    .lineLimit(2)
                HStack(spacing: 6) {
                    Text("\(event.cacheReadLabel)  \(Text(event.cacheReadHitRateLabel).font(.caption2.monospacedDigit()))")
                        .fixedSize(horizontal: false, vertical: true)
                        .help("缓存读取 Token / 总输入 Token；Anthropic 总输入包含缓存读取和缓存写入。数据未确认或分母未知时显示 —。")
                    if event.isInFlight {
                        Text("暂计")
                            .padding(.horizontal, 4)
                            .background(.quaternary, in: RoundedRectangle(cornerRadius: 3))
                    }
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }
        }
        .help(event.isInFlight
            ? "仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。"
            : "输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。")
    }
}

/// Column proportions for the run-page event rows. Live and history
/// lists share this layout so in-flight overlay and paged history stay aligned
/// without an NSTableView-backed `Table`.
private struct RuntimeEventColumnWidths {
    let request: CGFloat
    let route: CGFloat
    let result: CGFloat
    let duration: CGFloat
    let usage: CGFloat
    let message: CGFloat

    // 包含各列的左右内边距；低于可读下限时由容器切换为紧凑卡片。
    private static let minimums: [CGFloat] = [200, 166, 132, 172, 224, 150]
    static let minimumTableWidth = minimums.reduce(0, +) + 16

    static func resolve(availableWidth: CGFloat) -> RuntimeEventColumnWidths {
        let minimumTotal = minimums.reduce(0, +)
        // 结果列保持完整的三行，额外空间优先给名称、耗时和 Token。
        let weights: [CGFloat] = [0.28, 0.20, 0, 0.20, 0.24, 0.08]
        let remainder = max(0, availableWidth - minimumTotal)
        let widths = zip(minimums, weights).map { minimum, weight in
            minimum + remainder * weight
        }
        return RuntimeEventColumnWidths(
            request: widths[0],
            route: widths[1],
            result: widths[2],
            duration: widths[3],
            usage: widths[4],
            message: widths[5]
        )
    }
}

/// 「最近事件」面板:筛选 + 表格 + 选中详情。当前只挂在运行页；统计页只保留错误样本下钻。
struct RecentEventsPanel: View {
    let events: [RuntimeEvent]
    /// 实时进行中的事件单独显示，不计入稳定历史页的 totalCount/pageSize。
    var liveEvents: [RuntimeEvent] = []
    var showEventDate = true
    var hint = "客户端请求和上游尝试通过请求 ID 关联；HTTP 状态与最终结果分开显示，200 后的协议失败不会被误报为成功。"
    var detailedEvent: RuntimeEvent?
    var hasMore = false
    /// 稳定分页元数据；首次加载完成前为空。
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
    var initialOutcomeFilter: String?
    var onOutcomeFilterChange: ((String) -> Void)?
    var requestChainTruncated: Bool = false
    var requestChainEvents: [RuntimeEvent]?
    var requestChainLoading = false
    var onSelectEvent: ((String?) -> Void)?
    var sortAscendingPreference: Binding<Bool>?
    @State private var selectedEventID: String?
    @State private var expandedEventGroups: Set<String> = []
    @Environment(\.sumpterPalette) private var palette
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    // 默认只看客户端:一次请求一行。排 failover 时切「全部」看完整上游尝试链。
    @State private var eventKindFilter: RuntimeEventKindFilter = .client
    @State private var eventOutcomeFilter = ""
    @State private var sortAscending = false

    private var eventRowHeight: CGFloat {
        // 第一列收敛为最多三行(时间+项目 / 请求摘要 / 工具),留足呼吸空间。
        dynamicTypeSize.isAccessibilitySize ? 102 : 68
    }

    var body: some View {
        SectionPanel(title: "最近事件", hint: hint) {
            VStack(alignment: .leading, spacing: 8) {
                eventToolbar
                if !visibleEvents.isEmpty {
                    pageTelemetryStrip
                }
                eventListSummary
                if !visibleLiveEvents.isEmpty {
                    liveEventsSection
                }
                if visibleEvents.isEmpty && visibleLiveEvents.isEmpty {
                    if pageLoading {
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
                                truncated: requestChainTruncated,
                                selectedEventID: $selectedEventID
                            )
                            .frame(width: 320, alignment: .topLeading)
                            Divider()
                            RuntimeEventInspector(
                                event: event, chain: selectedRequestChain,
                                expanded: $expandedEventGroups,
                                onClose: { selectedEventID = nil }
                            )
                            .frame(minWidth: 380, maxWidth: .infinity, alignment: .topLeading)
                        }
                        VStack(alignment: .leading, spacing: 16) {
                            RuntimeRequestTraceView(
                                chain: selectedRequestChain,
                                truncated: requestChainTruncated,
                                selectedEventID: $selectedEventID,
                                maxHeight: 280
                            )
                            Divider()
                            RuntimeEventInspector(
                                event: event, chain: selectedRequestChain,
                                expanded: $expandedEventGroups,
                                onClose: { selectedEventID = nil }
                            )
                            .frame(maxWidth: .infinity, alignment: .topLeading)
                        }
                    }
                }
            }
        }
        .onAppear {
            if let sortAscendingPreference { sortAscending = sortAscendingPreference.wrappedValue }
            if let initialKindFilter { eventKindFilter = initialKindFilter }
            if let initialOutcomeFilter { eventOutcomeFilter = initialOutcomeFilter }
            ensureSelection()
        }
        .onChange(of: visibleEventIDs) { _, _ in ensureSelection() }
        .onChange(of: sortAscending) { _, value in sortAscendingPreference?.wrappedValue = value }
        .onChange(of: selectedEventID) { _, eventID in onSelectEvent?(eventID) }
        .onChange(of: eventKindFilter) { _, filter in
            onKindFilterChange?(filter)
        }
        .onChange(of: eventOutcomeFilter) { _, outcome in
            onOutcomeFilterChange?(outcome)
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
        .onChange(of: initialOutcomeFilter) { _, outcome in
            guard let outcome, outcome != eventOutcomeFilter else { return }
            eventOutcomeFilter = outcome
        }
    }

    private var liveEventsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            // 标题与「上游暂计」说明并进列头行:计数落在「请求」列,暂计提示落在「Token 用量」列,
            // 少一行占位,进行中区块和历史表的列头也保持同一节奏。
            ViewThatFits(in: .horizontal) {
                GeometryReader { proxy in
                    eventColumnHeader(
                        widths: RuntimeEventColumnWidths.resolve(availableWidth: proxy.size.width - 16),
                        liveCount: visibleLiveEvents.count
                    )
                }
                .frame(minWidth: RuntimeEventColumnWidths.minimumTableWidth)
                .frame(height: 28)
                compactLiveHeading
            }
            Divider().opacity(0.35)
            ForEach(Array(visibleLiveEvents.enumerated()), id: \.element.id) { index, event in
                Button {
                    selectedEventID = event.id
                } label: {
                    ViewThatFits(in: .horizontal) {
                        liveEventRow(event).frame(minWidth: RuntimeEventColumnWidths.minimumTableWidth)
                        compactEventRow(event)
                    }
                }
                .buttonStyle(.plain)
                .background(selectedEventID == event.id ? palette.brand.opacity(0.08) : .clear)
                .overlay(alignment: .leading) {
                    // 进行中行的左侧色条保持实色:滚动/刷新时不做玻璃与阴影,活动感交给状态 pill。
                    Rectangle()
                        .fill(RuntimeEventDisplay.statusColor(event))
                        .frame(width: 3)
                        .accessibilityHidden(true)
                }
                .accessibilityLabel("进行中请求：\(RuntimeEventDisplay.requestSummary(event))")
                .accessibilityAddTraits(selectedEventID == event.id ? .isSelected : [])
                if index < visibleLiveEvents.count - 1 { Divider().opacity(0.28) }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.inset.opacity(colorScheme == .dark ? 0.55 : 0.6))
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(palette.brand.opacity(0.32), lineWidth: 1.1)
                .phaseAnimator([false, true]) { rim, glowing in
                    rim.opacity(glowing ? 0.95 : 0.38)
                } animation: { _ in
                    .easeInOut(duration: 2.4)
                }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(visibleLiveEvents.count) 个进行中请求")
    }

    /// 窄窗口没有列头,进行中区块退回一行标题。
    private var compactLiveHeading: some View {
        HStack(spacing: 7) {
            RuntimeLiveBreathingDot(color: palette.brand)
            Text("进行中 · \(visibleLiveEvents.count)")
                .font(.caption.weight(.semibold))
            Spacer(minLength: 12)
            Text("上游暂计").font(.caption).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var eventsTableContainer: some View {
        ZStack(alignment: .topTrailing) {
            // Do not use SwiftUI `Table` here.  It is NSTableView-backed and
            // regularly keeps the column header while clipping every row away
            // after an SSE/live-overlay refresh — the run page then looks
            // like an empty history table under the in-flight stack.
            ViewThatFits(in: .horizontal) {
                eventsHistoryTable
                compactEventsList
            }
            .id(pageIdentity)
            .opacity(pageLoading ? 0.66 : 1)
            .transaction { transaction in
                transaction.animation = nil
            }

            if pageLoading {
                HStack(spacing: 6) {
                    ProgressView()
                        .controlSize(.small)
                    Text("正在加载历史页…")
                        .font(.caption)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
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
        "\(currentPage ?? 0)-\(pageSize ?? 0)-\(eventKindFilter.rawValue)-\(eventOutcomeFilter)-\(sortAscending)"
    }

    /// Filtering and paging stay above the event rows so navigation remains
    /// visible without scrolling through a long history page.
    @ViewBuilder
    private var eventToolbar: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) {
                eventKindPicker
                    .frame(width: 204)
                eventOutcomePicker
                    .frame(width: 256)
                eventSortButton
                Spacer(minLength: 8)
                eventPageControls(compact: false)
            }

            VStack(alignment: .leading, spacing: 8) {
                SumpterWrappingLayout(horizontalSpacing: 10, verticalSpacing: 8) {
                    eventKindPicker
                        .frame(width: 204)
                    eventOutcomePicker
                        .frame(width: 256)
                    eventSortButton
                }
                eventPageControls(compact: true)
            }
        }
    }

    @ViewBuilder
    private var pageTelemetryStrip: some View {
        let nonNotify = visibleEvents.filter { $0.kind != "notify" }
        if !nonNotify.isEmpty {
            let succeeded = nonNotify.filter { $0.outcome == .succeeded }.count
            let failed = nonNotify.filter { $0.outcome == .failed }.count
            let cancelled = nonNotify.filter { $0.outcome == .cancelled }.count
            let completed = succeeded + failed + cancelled
            let successRate = completed > 0
                ? String(format: "%.1f%%", Double(succeeded) / Double(completed) * 100)
                : "—"
            let ttfbValues = nonNotify.compactMap(\.ttfbMS).filter { $0 >= 0 }
            let avgTTFB = ttfbValues.isEmpty ? nil : Double(ttfbValues.reduce(0, +)) / Double(ttfbValues.count)
            let durationValues = nonNotify.map(\.durationMS).filter { $0 >= 0 }
            let avgDuration = durationValues.isEmpty ? nil : Double(durationValues.reduce(0, +)) / Double(durationValues.count)
            let totalTokens = nonNotify.reduce(0) { acc, ev in
                acc + max(0, ev.usageSummary?.inputTokens ?? 0) + max(0, ev.usageSummary?.outputTokens ?? 0)
            }
            let failoverCount = nonNotify.filter(\.failover).count

            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 185, maximum: 380), spacing: 10)],
                spacing: 10
            ) {
                telemetryOverviewCard(
                    title: "本页请求健康度",
                    value: successRate,
                    subLeft: "成功 \(succeeded)",
                    subRight: "失败 \(failed) · 取消 \(cancelled)"
                ) {
                    GeometryReader { geo in
                        let w = max(20, geo.size.width)
                        let goodW = completed > 0 ? w * CGFloat(succeeded) / CGFloat(completed) : 0
                        let failW = completed > 0 ? w * CGFloat(failed) / CGFloat(completed) : 0
                        let warnW = max(0, w - goodW - failW)
                        HStack(spacing: 1.5) {
                            if goodW > 0 { Capsule().fill(palette.success).frame(width: goodW, height: 5) }
                            if failW > 0 { Capsule().fill(palette.danger).frame(width: failW, height: 5) }
                            if warnW > 0 && completed > 0 { Capsule().fill(palette.warning).frame(width: warnW, height: 5) }
                        }
                    }
                    .frame(height: 5)
                }

                telemetryOverviewCard(
                    title: "本页首字节均值",
                    value: avgTTFB.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) } ?? "—",
                    subLeft: "蓝青段代表首字节响应",
                    subRight: "\(ttfbValues.count) 条已采样"
                )

                telemetryOverviewCard(
                    title: "本页完成耗时均值",
                    value: avgDuration.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) } ?? "—",
                    subLeft: "紫绿段代表流式生成",
                    subRight: "\(durationValues.count) 条已采样"
                )

                telemetryOverviewCard(
                    title: "本页 Token 与切换",
                    value: totalTokens > 0 ? RuntimeEventPresentation.tokenCountDisplay(totalTokens) : "—",
                    subLeft: "本页输入+输出累计",
                    subRight: "故障转移 \(failoverCount) 次"
                )
            }
            .padding(.vertical, 2)
        }
    }

    private func telemetryOverviewCard<Bar: View>(
        title: String,
        value: String,
        subLeft: String,
        subRight: String,
        @ViewBuilder bar: () -> Bar = { EmptyView() }
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(title)
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text(value)
                    .font(.callout.monospacedDigit().weight(.semibold))
                    .foregroundStyle(palette.textPrimary)
                    .lineLimit(1)
            }
            bar()
            HStack(spacing: 6) {
                Text(subLeft)
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text(subRight)
                    .lineLimit(1)
            }
            .font(.caption2.monospacedDigit())
            .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.inset.opacity(0.75), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }

    private var eventKindPicker: some View {
        Picker("类型", selection: $eventKindFilter) {
            ForEach(RuntimeEventKindFilter.allCases, id: \.self) { filter in
                Text(filter.title).tag(filter)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(minHeight: 30)
    }

    private var eventOutcomePicker: some View {
        Picker("结果状态", selection: $eventOutcomeFilter) {
            Text("全部状态").tag("")
            Text("成功").tag("succeeded")
            Text("失败").tag("failed")
            Text("已取消").tag("cancelled")
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(minHeight: 30)
    }

    private var eventSortButton: some View {
        Button {
            sortAscending.toggle()
        } label: {
            Label(sortAscending ? "本页最早优先" : "本页最新优先", systemImage: "arrow.up.arrow.down")
                .font(.caption.weight(.medium))
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .help("切换当前页事件时间排序")
    }

    private var eventCountLabel: some View {
        Text(totalCount.map { "本页 \(visibleEvents.count) 条 · 共 \($0) 条" } ?? "本页 \(visibleEvents.count) 条 · 共 \(events.count) 条")
            .font(.subheadline.monospacedDigit())
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .fixedSize(horizontal: true, vertical: false)
    }

    private var eventListSummary: some View {
        SumpterWrappingLayout(horizontalSpacing: 16, verticalSpacing: 6) {
            eventCountLabel
            if requestChainLoading {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("正在读取请求链…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
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
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .center, spacing: 6) {
                Text(RuntimeEventDisplay.time(event.timestamp, includeDate: showEventDate))
                    .font(.subheadline.monospacedDigit().weight(.semibold))
                    .fixedSize(horizontal: true, vertical: false)
                if let project = RuntimeEventPresentation.projectAttribution(
                    eventKind: event.kind, metadata: event.codexMetadata, declared: event.clientDeclared,
                    projectedName: event.projectName, projectedSource: event.projectSource, projectedLocalUser: event.localUser
                ) {
                    Text(project.replacingOccurrences(of: "项目: ", with: ""))
                        .font(.caption2.weight(.medium))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1.5)
                        .background(palette.borderSubtle.opacity(0.65), in: Capsule())
                }
            }
            Text([RuntimeEventDisplay.kind(event.kind), RuntimeEventDisplay.clientKind(event), event.agentSummaryLabel,
                  RuntimeEventDisplay.rowPurpose(event), event.sourceIP]
                .compactMap { $0 }.joined(separator: " · "))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            // 本次调用的工具直接列在第一列,不用点开详情;分页列表已带 toolCalls 投影。
            if event.kind != "notify", let tools = RuntimeEventDisplay.toolCalls(event) {
                HStack(spacing: 4) {
                    Image(systemName: "wrench.and.screwdriver")
                        .font(.system(size: 9))
                        .foregroundStyle(palette.brand.opacity(0.85))
                    Text(tools)
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .help("调用工具：\(tools)")
            }
        }
        .help(RuntimeEventDisplay.requestSummary(event))
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventRouteCell(_ event: RuntimeEvent) -> some View {
        let logical = RuntimeEventDisplay.logicalModel(event)
        let clientModel = RuntimeEventDisplay.displayedModelName(event.clientModel)
        let isMapped = clientModel != nil && logical != "—" && clientModel != logical
        return VStack(alignment: .leading, spacing: 3) {
            if event.kind == "notify" {
                Text(event.hookEvent ?? "通知").font(.subheadline)
            } else {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text(logical)
                        .font(.subheadline.weight(.medium))
                        .lineLimit(1)
                        .help(RuntimeEventDisplay.model(event))
                    if isMapped, let clientModel {
                        Text("← \(clientModel)")
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(palette.borderSubtle.opacity(0.6), in: RoundedRectangle(cornerRadius: 4, style: .continuous))
                            .help("客户端请求模型 \(clientModel) → 路由模型 \(logical)")
                    }
                }
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text(RuntimeEventDisplay.endpoint(event))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .help(RuntimeEventDisplay.endpoint(event))
                    if event.failover {
                        Text("故障转移")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(palette.warning)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(palette.warning.opacity(0.14), in: Capsule())
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventResultCell(_ event: RuntimeEvent) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            if event.kind == "notify" {
                Text("通知事件").font(.subheadline)
            } else {
                RuntimeEventStatusSummary(event: event, compact: true, includeUsage: false)
                Text(RuntimeEventPresentation.transportDisplay(event.transport))
                    .font(.caption2.weight(.medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventDurationCell(_ event: RuntimeEvent) -> some View {
        let ttfb = max(0, event.ttfbMS ?? 0)
        let total = max(ttfb, event.durationMS)
        let ttfbRatio = total > 0 ? min(1.0, max(0.1, Double(ttfb) / Double(total))) : 0
        return Group {
            if event.kind == "notify" {
                Text("—").foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 5) {
                    SumpterWrappingLayout(horizontalSpacing: 6, verticalSpacing: 3) {
                        RuntimeEventDurationText(event: event)
                            .font(.caption.monospacedDigit().weight(.medium))
                            .foregroundStyle(RuntimeEventDisplay.durationColor(event))
                            .lineLimit(1)
                            .fixedSize(horizontal: true, vertical: false)
                        if let speed = event.outputTokensPerSecond, event.isSucceeded, speed.isFinite, speed >= 0 {
                            Text(String(format: "%.1f tok/s", speed))
                                .font(.system(size: 10, weight: .semibold, design: .monospaced))
                                .foregroundStyle(palette.info)
                                .padding(.horizontal, 5)
                                .padding(.vertical, 1)
                                .background(palette.info.opacity(0.12), in: Capsule())
                                .fixedSize(horizontal: true, vertical: false)
                        } else if event.kind == "client" && !event.isInFlight {
                            Text("—").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                                .help("无法可靠测量输出速度")
                        }
                    }
                    if !event.isInFlight && total > 0 {
                        GeometryReader { geo in
                            let width = max(24, geo.size.width)
                            let ttfbW = ttfb > 0 ? max(4, width * ttfbRatio) : 0
                            let streamW = max(0, width - ttfbW)
                            HStack(spacing: 1) {
                                if ttfbW > 0 {
                                    RoundedRectangle(cornerRadius: 2, style: .continuous)
                                        .fill((event.ttfbMS ?? 0) >= 5_000 ? palette.warning : palette.info)
                                        .frame(width: ttfbW, height: 4)
                                }
                                if streamW > 0 {
                                    RoundedRectangle(cornerRadius: 2, style: .continuous)
                                        .fill(palette.brand.opacity(0.85))
                                        .frame(width: streamW, height: 4)
                                }
                            }
                        }
                        .frame(height: 4)
                    }
                }
                .help(RuntimeEventDisplay.durationHelp(event))
            }
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventMessageCell(_ event: RuntimeEvent) -> some View {
        let friendly = RuntimeEventDisplay.friendlyMessage(event)
        return VStack(alignment: .leading, spacing: 2) {
            Text(friendly.isEmpty ? "-" : friendly)
                .foregroundStyle(RuntimeEventDisplay.messageColor(event, friendly: friendly))
                .font(.subheadline)
                .lineLimit(2)
                .help(
                    event.failureDetail
                        ?? RuntimeEventPresentation.messageForDisplay(
                            event.message,
                            clientKind: event.clientKind
                        )
                        ?? friendly
                )
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func eventUsageCell(_ event: RuntimeEvent) -> some View {
        Group {
            if event.kind == "notify" {
                Text("—").foregroundStyle(.secondary)
            } else {
                RuntimeEventUsageSummary(event: event)
            }
        }
        .frame(maxWidth: .infinity, minHeight: eventRowHeight - 6, alignment: .leading)
    }

    private func liveEventRow(_ event: RuntimeEvent) -> some View {
        GeometryReader { proxy in
            wideEventRow(
                event,
                widths: RuntimeEventColumnWidths.resolve(
                    availableWidth: proxy.size.width - 16
                )
            )
            .frame(width: proxy.size.width, height: proxy.size.height, alignment: .leading)
        }
        .frame(height: eventRowHeight)
        .contentShape(Rectangle())
    }

    private func wideEventRow(_ event: RuntimeEvent, widths: RuntimeEventColumnWidths) -> some View {
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
            eventDurationCell(event)
                .padding(.horizontal, 12)
                .frame(width: widths.duration, alignment: .leading)
            eventUsageCell(event)
                .padding(.horizontal, 12)
                .frame(width: widths.usage, alignment: .leading)
            eventMessageCell(event)
                .padding(.horizontal, 12)
                .frame(width: widths.message, alignment: .leading)
        }
        .padding(.horizontal, 8)
        .frame(height: eventRowHeight)
        .contentShape(Rectangle())
    }

    /// `liveCount` 非空时是进行中区块的列头:「请求」列改成计数标题,「Token 用量」列带上暂计提示。
    private func eventColumnHeader(widths: RuntimeEventColumnWidths, liveCount: Int? = nil) -> some View {
        HStack(alignment: .center, spacing: 0) {
            if let liveCount {
                HStack(spacing: 7) {
                    RuntimeLiveBreathingDot(color: palette.brand)
                    Text("进行中 · \(liveCount)")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(palette.textPrimary)
                }
                .padding(.horizontal, 12)
                .frame(width: widths.request, alignment: .leading)
            } else {
                columnHeaderLabel("请求", width: widths.request)
            }
            columnHeaderLabel("模型 / 路由", width: widths.route)
            columnHeaderLabel("结果", width: widths.result)
            columnHeaderLabel("首字节 → 总耗时", width: widths.duration)
            if liveCount != nil {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text("Token 用量").font(.caption.weight(.semibold))
                    Text("上游暂计").font(.caption2)
                        .foregroundStyle(.tertiary)
                        .help("仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。")
                }
                .padding(.horizontal, 12)
                .frame(width: widths.usage, alignment: .leading)
            } else {
                columnHeaderLabel("Token 用量", width: widths.usage)
            }
            columnHeaderLabel("事件状态", width: widths.message)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .foregroundStyle(.secondary)
    }

    private func columnHeaderLabel(_ title: String, width: CGFloat) -> some View {
        Text(title)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 12)
            .frame(width: width, alignment: .leading)
    }

    /// Narrow windows use a readable vertical summary instead of forcing the
    /// desktop columns into a horizontal scroll region.  The same row
    /// remains selectable and opens the full detail view, so no capability is
    /// lost at the compact breakpoint.
    private func compactEventRow(_ event: RuntimeEvent) -> some View {
        let logical = RuntimeEventDisplay.logicalModel(event)
        let clientModel = RuntimeEventDisplay.displayedModelName(event.clientModel)
        let isMapped = clientModel != nil && logical != "—" && clientModel != logical
        return VStack(alignment: .leading, spacing: 5) {
            SumpterWrappingLayout(horizontalSpacing: 12, verticalSpacing: 6) {
                Text(RuntimeEventDisplay.time(event.timestamp, includeDate: showEventDate))
                    .font(.subheadline.monospacedDigit())
                RuntimeEventStatusSummary(event: event, compact: true, includeUsage: false)
                RuntimeEventDurationText(event: event)
                    .font(.caption.monospacedDigit())
                    .lineLimit(1)
                    .fixedSize(horizontal: true, vertical: false)
                if let speed = event.outputTokensPerSecond, event.isSucceeded, speed.isFinite, speed >= 0 {
                    Text(String(format: "%.1f tok/s", speed))
                        .font(.system(size: 10, weight: .semibold, design: .monospaced))
                        .foregroundStyle(palette.info)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(palette.info.opacity(0.12), in: Capsule())
                        .fixedSize(horizontal: true, vertical: false)
                } else if event.kind == "client" && !event.isInFlight {
                    Text("—").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                        .help("无法可靠测量输出速度")
                }
            }
            SumpterWrappingLayout(horizontalSpacing: 6, verticalSpacing: 4) {
                Text("\(logical) · \(RuntimeEventDisplay.endpoint(event))")
                    .font(.subheadline.weight(.medium))
                    .lineLimit(2)
                    .foregroundStyle(palette.textPrimary)
                if isMapped, let clientModel {
                    Text("← \(clientModel)")
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(palette.borderSubtle.opacity(0.6), in: RoundedRectangle(cornerRadius: 4, style: .continuous))
                }
                if event.failover {
                    Text("故障转移")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(palette.warning)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(palette.warning.opacity(0.14), in: Capsule())
                }
            }
            Text(RuntimeEventDisplay.requestSummary(event))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .help(RuntimeEventDisplay.requestSummary(event))
            if event.kind != "notify", let tools = RuntimeEventDisplay.toolCalls(event) {
                HStack(spacing: 4) {
                    Image(systemName: "wrench.and.screwdriver")
                        .font(.system(size: 9))
                        .foregroundStyle(palette.brand.opacity(0.85))
                    Text(tools)
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            if event.kind != "notify" {
                RuntimeEventUsageSummary(event: event)
            }
            let friendly = RuntimeEventDisplay.friendlyMessage(event)
            if !friendly.isEmpty {
                Text("事件状态 · \(friendly)")
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
                        selectedEventID = (selectedEventID == event.id ? nil : event.id)
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

    private var eventsHistoryTable: some View {
        GeometryReader { proxy in
            let widths = RuntimeEventColumnWidths.resolve(
                availableWidth: proxy.size.width - 16
            )
            VStack(spacing: 0) {
                eventColumnHeader(widths: widths)
                Divider().opacity(0.35)
                ScrollView(.vertical, showsIndicators: true) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(visibleEvents) { event in
                            Button {
                                selectedEventID = (selectedEventID == event.id ? nil : event.id)
                            } label: {
                                wideEventRow(event, widths: widths)
                            }
                            .buttonStyle(.plain)
                            .background(selectedEventID == event.id ? palette.brand.opacity(0.08) : .clear)
                            .accessibilityLabel("事件：\(RuntimeEventDisplay.requestSummary(event))")
                            .accessibilityAddTraits(selectedEventID == event.id ? .isSelected : [])
                            if event.id != visibleEvents.last?.id {
                                Divider().opacity(0.28)
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .frame(minWidth: RuntimeEventColumnWidths.minimumTableWidth)
        .frame(maxWidth: .infinity)
        .frame(height: eventTableHeight)
        .background(palette.inset)
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        }
    }

    /// 事件表当前可见的行:按类型/结果筛选 + 按时间正序/倒序。
    private var visibleEvents: [RuntimeEvent] {
        let base = RuntimeEvent.ordered(events, filter: eventKindFilter).filter { event in
            guard !eventOutcomeFilter.isEmpty else { return true }
            return event.outcome?.rawValue == eventOutcomeFilter
        }
        return sortAscending ? Array(base.reversed()) : base
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
        return (events + liveEvents + (requestChainEvents ?? [])).first { $0.id == selectedEventID }
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
        return Swift.min(560, Swift.max(180, 34 + CGFloat(reservedCount) * eventRowHeight))
    }

    private func ensureSelection() {
        guard !visibleEventIDs.isEmpty else {
            selectedEventID = nil
            return
        }
        // 请求链允许在“仅客户端”筛选下点选隐藏的上游尝试。SSE 新事件到达会改变
        // visibleEventIDs；只要当前详情事件仍存在，就不能把选择重置回客户端行。
        if selectedEvent != nil {
            return
        }
        // 首次进入运行页不自动打开第一条详情；详情只在用户明确点选
        // 一行后展开，避免实时刷新把用户的注意力强行带到新事件。
        selectedEventID = nil
    }
}

/// 同一 requestID 的完整上游尝试链。默认筛选只看客户端行时也能直接看到重试/failover。
struct RuntimeRequestTraceView: View {
    let chain: [RuntimeEvent]
    var truncated: Bool = false
    @Binding var selectedEventID: String?
    var maxHeight: CGFloat = 440
    @State private var contentHeight: CGFloat = 440
    @Environment(\.sumpterPalette) private var palette

    private var clientEvent: RuntimeEvent? { chain.first { $0.kind == "client" } }
    private var upstreamAttempts: [RuntimeEvent] {
        Array(chain.filter { $0.kind == "upstream" }.reversed())
    }
    private var maxChainDurationMS: Int {
        max(1, chain.map(\.durationMS).max() ?? 1)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text("请求链")
                    .font(.callout.weight(.semibold))
                Spacer()
                Text("\(truncated ? "至少 " : "")\(upstreamAttempts.count) 次上游尝试\(truncated ? "（已截断）" : "")")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            ScrollView(.vertical, showsIndicators: true) {
                traceContent
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.trailing, 8)
                    .onGeometryChange(for: CGFloat.self) { proxy in
                        proxy.size.height
                    } action: { height in
                        contentHeight = height
                    }
            }
            .frame(height: min(contentHeight, maxHeight))
            .scrollBounceBehavior(.basedOnSize)
            .id(clientEvent?.requestID ?? chain.first?.requestID ?? chain.first?.id)
            .accessibilityLabel("请求链事件列表")
        }
    }

    @ViewBuilder
    private func chainWaterfallBar(for event: RuntimeEvent) -> some View {
        let duration = max(0, event.durationMS)
        if event.kind != "notify", duration > 0 {
            let ttfb = event.ttfbMS.map { min(duration, max(0, $0)) }
            let totalRatio = min(1.0, max(0.06, Double(duration) / Double(max(duration, maxChainDurationMS))))
            let ttfbShare = ttfb.map { min(1.0, max(0.0, Double($0) / Double(duration))) } ?? 0
            VStack(alignment: .leading, spacing: 3) {
                GeometryReader { geo in
                    let barWidth = max(8, geo.size.width * totalRatio)
                    let ttfbWidth = barWidth * ttfbShare
                    ZStack(alignment: .leading) {
                        Capsule()
                            .fill(palette.borderSubtle.opacity(0.6))
                            .frame(height: 5)
                        HStack(spacing: 0) {
                            if ttfb != nil, ttfbWidth > 0 {
                                Rectangle()
                                    .fill(palette.info.opacity(0.85))
                                    .frame(width: ttfbWidth, height: 5)
                            }
                            Rectangle()
                                .fill(RuntimeEventDisplay.statusColor(event).opacity(0.85))
                                .frame(width: max(0, barWidth - ttfbWidth), height: 5)
                        }
                        .clipShape(Capsule())
                    }
                }
                .frame(height: 5)
                Text(RuntimeEventPresentation.durationWithTTFB(
                    ttfbMS: event.ttfbMS,
                    durationMS: event.durationMS,
                    inFlight: event.isInFlight,
                    startedAt: event.timestamp,
                    now: Date()
                ))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
            }
            .help("首字节等待（青色）与流式响应耗时（状态色）占比")
        }
    }

    private var traceContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let clientEvent {
                Button {
                    selectedEventID = clientEvent.id
                } label: {
                    VStack(alignment: .leading, spacing: 5) {
                        HStack(spacing: 8) {
                            Label(RuntimeEventDisplay.clientKind(clientEvent), systemImage: "desktopcomputer")
                                .font(.caption.weight(.semibold))
                            Spacer()
                        }
                        RuntimeEventStatusSummary(event: clientEvent, compact: true)
                        chainWaterfallBar(for: clientEvent)
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
                    .padding(8)
                    .background(
                        clientEvent.id == selectedEventID ? palette.brand.opacity(0.08) : Color.clear,
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous)
                    )
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
                VStack(alignment: .leading, spacing: 4) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(RuntimeEventDisplay.endpoint(event))
                            .font(.callout.weight(event.id == selectedEventID ? .semibold : .medium))
                            .fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 6)
                    }
                    RuntimeEventStatusSummary(event: event, compact: true)
                    chainWaterfallBar(for: event)
                    Text([
                        RuntimeEventDisplay.model(event),
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
            .padding(6)
            .background(
                event.id == selectedEventID ? palette.brand.opacity(0.08) : Color.clear,
                in: RoundedRectangle(cornerRadius: 8, style: .continuous)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("查看第 \(number) 次上游尝试详情")
    }
}

/// 选中事件按“操作摘要 → 诊断字段”分层。首屏只放判断请求是否正常所需的信息，
/// 长 ID、流计时与完整 Codex 元数据渐进披露，避免把所有采集字段铺成信息墙。

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
