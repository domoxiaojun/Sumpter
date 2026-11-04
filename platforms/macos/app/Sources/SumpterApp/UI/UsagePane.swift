import AppKit
import SumpterCore
import SwiftUI

private extension AdminWire.RuntimeDimensionRow {
    var costValueForSorting: Int { cost?.estimatedCostMicros ?? -1 }
    /// The success-rate denominator follows the visible dashboard contract:
    /// terminal success + terminal failure + client cancellation.  Pending
    /// requests are intentionally excluded instead of being presented as a
    /// failed outcome.
    var successRateForSorting: Double {
        let completed = successes + failures + cancelled
        guard completed > 0 else { return -1 }
        return Double(successes) / Double(completed)
    }

    /// Keep rows with no measured duration together at the end of an
    /// ascending sort while preserving the server's numeric ordering for
    /// observed values.
    var averageDurationForSorting: Double { averageDurationMS ?? -1 }
    var outputSpeedForSorting: Double { outputTokensPerSecond ?? -1 }
}

private struct RecreateDatabaseButton: View {
    @ObservedObject var model: AppModel
    @State private var isConfirming = false

    var body: some View {
        Button(role: .destructive) {
            isConfirming = true
        } label: {
            Label("重置并新建数据库", systemImage: "arrow.triangle.2.circlepath")
        }
        .controlSize(.small)
        .help("删除旧版 SQLite 结构并按当前版本新建；不会删除诊断捕获")
        .confirmationDialog("重置并新建数据库？", isPresented: $isConfirming) {
            Button("重置并新建数据库", role: .destructive) {
                model.recreateRuntimeStats()
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("会删除全部 SQLite 运行统计，并按当前版本重新创建 runtime.sqlite3。旧版自动清理字段会被移除，诊断捕获不会删除；此操作无法撤销。")
        }
    }
}


private struct AttributionFacetPicker: View {
    private func label(_ value: String) -> String {
        switch title {
        case "请求类型": RuntimeEventPresentation.observedRequestKind(value)
        case "客户端形态": RuntimeEventPresentation.clientVariant(value)
        case "代理角色": RuntimeEventPresentation.agentRole(value)
        default: value
        }
    }
    let title: String
    @Binding var selection: String
    let rows: [AdminWire.RuntimeAnalytics.FacetRow]
    var body: some View {
        Picker(title, selection: $selection) {
            Text("全部").tag("")
            ForEach(rows) { item in Text("\(label(item.value)) · \(item.count)").tag(item.value) }
        }
    }
}

private struct RuntimeCleanupSheet: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.sumpterPalette) private var palette
    @State private var selection = "30"
    @State private var customDate = Date()
    @State private var preview: AdminWire.RuntimeCleanupPreview?
    @State private var loading = false
    @State private var error = ""

    private enum CleanupRange: String, CaseIterable, Identifiable {
        case sevenDays = "7"
        case thirtyDays = "30"
        case ninetyDays = "90"
        case custom
        case all

        var id: String { rawValue }

        var title: String {
            switch self {
            case .sevenDays: "早于 7 天"
            case .thirtyDays: "早于 30 天"
            case .ninetyDays: "早于 90 天"
            case .custom: "自定义日期"
            case .all: "全部已完成记录"
            }
        }

        var detail: String {
            switch self {
            case .sevenDays: "仅清理一周以前的数据"
            case .thirtyDays: "仅清理一个月以前的数据"
            case .ninetyDays: "仅清理三个月以前的数据"
            case .custom: "按指定日期作为截止点"
            case .all: "清理所有已完成的请求组"
            }
        }

        var systemImage: String {
            switch self {
            case .sevenDays: "calendar"
            case .thirtyDays: "calendar.badge.clock"
            case .ninetyDays: "calendar.badge.exclamationmark"
            case .custom: "calendar.badge.plus"
            case .all: "trash"
            }
        }

        var isDestructive: Bool { self == .all }
    }

    private var selectedRange: CleanupRange {
        CleanupRange(rawValue: selection) ?? .thirtyDays
    }

    private var olderThan: Double {
        let now = Date().timeIntervalSinceReferenceDate
        switch selection {
        case "all": return now + 1
        case "custom": return Calendar.current.startOfDay(for: customDate).timeIntervalSinceReferenceDate
        default: return now - (Double(selection) ?? 30) * 86_400
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 24) {
                    rangeSection
                    protectionSection
                    previewSection
                    if !error.isEmpty {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .font(.callout)
                            .foregroundStyle(palette.danger)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(palette.danger.opacity(0.10), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    }
                }
                .padding(.horizontal, 28)
                .padding(.vertical, 24)
            }
            .frame(maxHeight: 560)
            Divider()
            footer
        }
        .background(palette.surface)
        .frame(width: 720)
        .frame(minHeight: 560, idealHeight: 620, maxHeight: 720)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 14) {
            ZStack {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(palette.active)
                Image(systemName: "externaldrive.badge.timemachine")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(palette.brand)
            }
            .frame(width: 46, height: 46)

            VStack(alignment: .leading, spacing: 5) {
                Text("清理运行统计")
                    .font(.title2.weight(.semibold))
                Text("按时间移除已完成的请求组，释放历史统计占用。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 12)
            Button(action: { if !loading { dismiss() } }) {
                Image(systemName: "xmark")
                    .font(.body.weight(.semibold))
                    .frame(width: 30, height: 30)
            }
            .buttonStyle(.borderless)
            .foregroundStyle(.secondary)
            .disabled(loading)
            .help("关闭")
            .accessibilityLabel("关闭清理窗口")
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 22)
    }

    private var rangeSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("1. 选择清理范围")
                        .font(.headline)
                    Text("只会处理已经完成的请求组；进行中的请求始终保留。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 12)
                Text("默认 30 天")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(palette.brand)
            }

            LazyVGrid(
                columns: [GridItem(.flexible(minimum: 160)), GridItem(.flexible(minimum: 160)), GridItem(.flexible(minimum: 160))],
                spacing: 10
            ) {
                ForEach(CleanupRange.allCases.filter { !$0.isDestructive }) { range in
                    rangeOption(range)
                }
            }
            rangeOption(.all)

            if selectedRange == .custom {
                VStack(alignment: .leading, spacing: 9) {
                    Text("截止日期")
                        .font(.subheadline.weight(.medium))
                    DatePicker(
                        "清理早于此日期的记录",
                        selection: $customDate,
                        in: ...Date(),
                        displayedComponents: .date
                    )
                    .datePickerStyle(.field)
                    .accessibilityLabel("清理早于此日期的记录")
                    .onChange(of: customDate) { _, _ in
                        preview = nil
                        error = ""
                    }
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(palette.inset, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .stroke(palette.borderSubtle, lineWidth: 0.8)
                )
            }

            if selectedRange == .all {
                Label("这是批量清理操作，但不会删除进行中的请求、诊断捕获或数据库结构。需要回收数据库文件时，请使用“重置并新建数据库”。", systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(palette.warning)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(palette.warning.opacity(0.10), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
        }
    }

    private func rangeOption(_ range: CleanupRange) -> some View {
        let isSelected = selectedRange == range
        let accent = range.isDestructive ? palette.danger : palette.brand
        return Button {
            guard !loading else { return }
            selection = range.rawValue
            preview = nil
            error = ""
        } label: {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: range.systemImage)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(isSelected ? accent : .secondary)
                    .frame(width: 20)
                VStack(alignment: .leading, spacing: 3) {
                    Text(range.title)
                        .font(.body.weight(.medium))
                        .foregroundStyle(.primary)
                    Text(range.detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
                    .font(.title3)
                    .foregroundStyle(isSelected ? accent : palette.borderStrong)
            }
            .padding(13)
            .frame(maxWidth: .infinity, minHeight: 76, alignment: .leading)
            .background(isSelected ? accent.opacity(0.10) : palette.inset, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .stroke(isSelected ? accent : palette.borderSubtle, lineWidth: isSelected ? 1.2 : 0.8)
            )
        }
        .buttonStyle(.plain)
        .disabled(loading)
        .accessibilityLabel(range.title)
        .accessibilityValue(isSelected ? "已选择，\(range.detail)" : range.detail)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    private var protectionSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("清理边界")
                .font(.headline)
            LazyVGrid(
                columns: [GridItem(.flexible(minimum: 180)), GridItem(.flexible(minimum: 180)), GridItem(.flexible(minimum: 180))],
                spacing: 8
            ) {
                protectionItem("整组完成才会删除", systemImage: "checkmark.circle", color: palette.success)
                protectionItem("进行中请求保留", systemImage: "arrow.triangle.2.circlepath", color: palette.info)
                protectionItem("诊断捕获保留", systemImage: "waveform.path.ecg", color: palette.info)
                protectionItem("数据库结构不变", systemImage: "externaldrive", color: palette.brand)
                protectionItem("自动保留策略不变", systemImage: "clock.arrow.circlepath", color: palette.warning)
            }
        }
    }

    private func protectionItem(_ title: String, systemImage: String, color: Color) -> some View {
        Label(title, systemImage: systemImage)
            .font(.caption)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
            .padding(.horizontal, 10)
            .background(palette.inset, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(palette.borderSubtle, lineWidth: 0.8)
            )
            .symbolRenderingMode(.hierarchical)
            .tint(color)
    }

    private var previewSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("2. 预览影响")
                        .font(.headline)
                    Text(preview == nil ? "先预览再执行，避免误删。" : "以下是按当前范围计算的结果。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                Image(systemName: preview == nil ? "arrow.down.circle" : "checkmark.circle.fill")
                    .foregroundStyle(preview == nil ? .secondary : palette.success)
            }

            if let preview {
                HStack(spacing: 10) {
                    previewMetric("预计删除", value: tokenNumber(preview.deletableEvents), detail: "条事件", color: palette.danger)
                    previewMetric("涉及请求", value: tokenNumber(preview.deletableRequests), detail: "个请求组", color: palette.brand)
                    previewMetric("清理后保留", value: tokenNumber(preview.remainingEvents), detail: "条事件", color: palette.success)
                }
                if preview.deletableEvents == 0 {
                    Label("当前范围没有可清理的事件。", systemImage: "checkmark.circle.fill")
                        .font(.caption)
                        .foregroundStyle(palette.success)
                }
            } else {
                Label("点击底部“预览清理范围”后，这里会显示预计删除量和保留量。", systemImage: "info.circle")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(16)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(palette.inset, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
        }
    }

    private func previewMetric(_ title: String, value: String, detail: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.title3.monospacedDigit().weight(.semibold))
                .foregroundStyle(color)
            Text(detail)
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, minHeight: 86, alignment: .leading)
        .padding(.horizontal, 14)
        .background(palette.raised, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }

    private var footer: some View {
        HStack(spacing: 12) {
            if loading {
                ProgressView()
                    .controlSize(.small)
                Text(preview == nil ? "正在计算清理范围…" : "正在清理统计…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            Button("取消", action: dismiss.callAsFunction)
                .keyboardShortcut(.cancelAction)
                .disabled(loading)
            if let preview {
                Button(role: .destructive, action: performCleanup) {
                    Label("确认清理 \(tokenNumber(preview.deletableEvents)) 条", systemImage: "trash")
                }
                .buttonStyle(.borderedProminent)
                .tint(palette.danger)
                .keyboardShortcut(.defaultAction)
                .disabled(loading || preview.deletableEvents <= 0)
            } else {
                Button(action: requestPreview) {
                    Label("预览清理范围", systemImage: "sparkles.rectangle.stack")
                }
                .buttonStyle(.borderedProminent)
                .tint(palette.brand)
                .keyboardShortcut(.defaultAction)
                .disabled(loading)
            }
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 18)
    }

    private func tokenNumber(_ value: Int) -> String {
        value.formatted(.number.grouping(.automatic))
    }

    private func requestPreview() {
        loading = true; error = ""; preview = nil
        Task {
            do { preview = try await model.previewRuntimeCleanup(olderThan: olderThan) }
            catch let nextError { self.error = nextError.localizedDescription }
            loading = false
        }
    }

    private func performCleanup() {
        guard preview != nil else { return }
        loading = true; error = ""
        Task {
            do { _ = try await model.cleanupRuntime(olderThan: olderThan); dismiss() }
            catch let nextError { self.error = nextError.localizedDescription }
            loading = false
        }
    }
}

struct UsagePane: View {
    @Environment(\.sumpterPalette) private var palette
    private enum StatisticsBoard: String, CaseIterable, Identifiable {
        case overview
        case trends
        case tokens
        case errors

        var id: String { rawValue }

        var title: String {
            switch self {
            case .overview: "概览"
            case .trends: "趋势"
            case .tokens: "成本"
            case .errors: "错误"
            }
        }

        var systemImage: String {
            switch self {
            case .overview: "rectangle.grid.2x2"
            case .trends: "chart.xyaxis.line"
            case .tokens: "number"
            case .errors: "exclamationmark.triangle"
            }
        }
    }

    private enum DiagnosticDimension: String, CaseIterable, Identifiable {
        case failures
        case routing
        case tools
        case stream
        case codex

        var id: String { rawValue }
        var title: String {
            switch self {
            case .failures: "失败"
            case .routing: "路由"
            case .tools: "工具"
            case .stream: "流"
            case .codex: "Codex"
            }
        }
    }

    private enum TrendMetric: String, CaseIterable, Identifiable {
        case clientRequests
        case successRate
        case failureRate
        case ttfbAverage
        case durationAverage
        case outputSpeed
        case outputTokens
        case cacheTokenRate
        case failovers
        case estimatedCost

        var id: String { rawValue }

        var title: String {
            switch self {
            case .clientRequests: "请求量"
            case .successRate: "成功率（比例）"
            case .failureRate: "失败率（比例）"
            case .ttfbAverage: "平均首字节"
            case .durationAverage: "平均完成耗时"
            case .outputSpeed: "输出速度"
            case .outputTokens: "输出 Token"
            case .cacheTokenRate: "缓存 Token 读占比（比例）"
            case .failovers: "故障转移"
            case .estimatedCost: "估算成本"
            }
        }

        var shortTitle: String {
            switch self {
            case .clientRequests: "请求量"
            case .successRate: "成功率"
            case .failureRate: "失败率"
            case .ttfbAverage: "平均首字节"
            case .durationAverage: "平均完成耗时"
            case .outputSpeed: "输出速度"
            case .outputTokens: "输出 Token"
            case .cacheTokenRate: "缓存 Token 读占比"
            case .failovers: "故障转移"
            case .estimatedCost: "估算成本"
            }
        }

        var keepsNilForEmptyBucket: Bool {
            switch self {
            case .successRate, .failureRate, .ttfbAverage, .durationAverage, .outputSpeed, .cacheTokenRate:
                true
            case .clientRequests, .outputTokens, .failovers, .estimatedCost:
                false
            }
        }
    }

    @ObservedObject var model: AppModel
    /// Keep the overview KPI groups on one visual baseline.  The value is a
    /// minimum rather than a fixed frame so larger Dynamic Type sizes can
    /// still expand cards without clipping their details.
    private let overviewMetricMinimumHeight: CGFloat = 112
    @State private var cleanupPresented = false
    @State private var pricingSheetPresented = false
    @State private var trendMetric: TrendMetric = .clientRequests
    @State private var hoveredTrendBucketIndex: Int?
    @State private var selectedErrorSampleEventID: String?
    @State private var expandedErrorEventGroups: Set<String> = []
    // 关闭自动刷新时冻结的画面快照;nil 表示跟随实时数据(后台轮询仍在更新模型)。
    @State private var frozenRuntime: RuntimeSnapshot?
    @State private var frozenHealth: ProxyHealthSummary?
    // 排行表的排序由用户掌握,默认是恒定的名称序 —— 见 UsageAggregateRow.stableOrder。
    @State private var failureSort = UsageAggregateRow.stableOrder
    @State private var protocolRouteSort = UsageAggregateRow.stableOrder
    @State private var featureRuleSort = UsageAggregateRow.stableOrder
    @State private var upstreamStatusSort = UsageAggregateRow.stableOrder
    @State private var toolSort = UsageAggregateRow.stableOrder
    @State private var streamSort = UsageAggregateRow.stableOrder
    @State private var codexSort = UsageAggregateRow.stableOrder
    @State private var runtimeEndpointTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var runtimeProjectTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var runtimeSessionTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var runtimeModelTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var diagnosticDimension: DiagnosticDimension = .failures
    @State private var confirmDeleteSessionID: String?
    @State private var confirmUnidentifiedSessionID: String?
    @State private var unidentifiedConfirmationPhrase = ""
    @State private var confirmStickyClearTarget: (key: String, name: String, isSession: Bool)?
    @State private var stickyClearingKey: String?
    @State private var locatingProjectName: String?
    @State private var confirmStoredExport = false
    @State private var exportPrivacy = "stored"
    @State private var exportScope = "events"
    @State private var exportFormat = "jsonl"
    @State private var confirmStoredEstimate = false
    @State private var selectedBoard: StatisticsBoard = .overview
    @State private var advancedFiltersExpanded = false
    @State private var exportExpanded = false
    @State private var storageSettingsPresented = false
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    private var displayRuntime: RuntimeSnapshot { frozenRuntime ?? model.runtime }
    private var displayHealth: ProxyHealthSummary { frozenHealth ?? model.health }
    private var analytics: AdminWire.RuntimeAnalytics? { model.runtimeAnalytics }
    private var facets: AdminWire.RuntimeAnalytics.Facets? {
        model.runtimeFacets?.facets ?? analytics?.facets
    }
    private var diagnosticSummary: RuntimeDiagnosticSummary {
        RuntimeDiagnosticSummary(events: displayRuntime.recentEvents)
    }

    private var unidentifiedSessionAlertPresented: Binding<Bool> {
        Binding(
            get: { confirmUnidentifiedSessionID != nil },
            set: { presented in
                guard !presented else { return }
                confirmUnidentifiedSessionID = nil
                unidentifiedConfirmationPhrase = ""
            }
        )
    }

    private var deleteSessionDialogPresented: Binding<Bool> {
        Binding(
            get: { confirmDeleteSessionID != nil },
            set: { presented in
                if !presented { confirmDeleteSessionID = nil }
            }
        )
    }

    private var historyEvents: [RuntimeEvent] {
        guard let page = model.runtimeHistoryPage else { return displayRuntime.recentEvents }
        let existing = RuntimeEvent.indexedByID(displayRuntime.recentEvents)
        let pageIDs = Set(page.events.map(\.id))
        let pageEvents = page.events.map { $0.mergedRuntimeEvent(with: existing[$0.id]) }

        // The v2 page is a stable historical snapshot, while SSE/in-flight
        // events continue to arrive in `runtime`.  On the first page keep
        // those newer live rows visible instead of making the statistics view
        // appear stale.  Later pages stay pure so server pagination metadata
        // remains meaningful and live rows are not duplicated on every page.
        guard page.page == 1 else { return pageEvents }
        let newestSnapshotTimestamp = pageEvents.map(\.timestamp).max() ?? .distantPast
        // RuntimeEvent (the legacy/SSE model) does not carry the SQLite seq.
        // Timestamp is the safe compatibility boundary: retain in-flight rows
        // and completed rows that arrived after the snapshot, but do not append
        // older cache rows that merely fell outside this page's pageSize.
        let liveEvents = displayRuntime.recentEvents.filter {
            !pageIDs.contains($0.id)
                && ($0.isInFlight || $0.timestamp > newestSnapshotTimestamp)
        }
        return (pageEvents + liveEvents).sorted { lhs, rhs in
            lhs.timestamp > rhs.timestamp
        }
    }

    private var v2RequestChainEvents: [RuntimeEvent]? {
        guard let chain = model.runtimeRequestChain else { return nil }
        let existing = RuntimeEvent.indexedByID(historyEvents)
        return chain.events.map { $0.mergedRuntimeEvent(with: existing[$0.id]) }
    }

    /// 开关或间隔一变就重启刷新循环。
    private var autoRefreshTaskID: String {
        "\(model.autoRefreshEnabled)-\(model.statisticsAutoRefreshIntervalSeconds)"
    }

    var body: some View {
        SettingsPage(title: "统计", subtitle: "请求统计") {
            if let issue = model.runtimeSummary?.startupIssue {
                VStack(alignment: .leading, spacing: 8) {
                    Label("运行统计数据库需要重建", systemImage: "exclamationmark.triangle.fill").font(.headline)
                    Text(issue.message).font(.callout).foregroundStyle(.secondary)
                    RecreateDatabaseButton(model: model)
                }.padding(14).background(Color.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
            }
            analyticsControlsPanel
            selectedBoardContent
        }
        .task(id: autoRefreshTaskID) {
            // 同 OverviewPane:数据靠 SSE + 兜底轮询,这里只处理冻结/解冻。
            model.setStatisticsVisible(true)
            model.setStatisticsBoard(selectedBoard.rawValue)
            guard model.autoRefreshEnabled else {
                frozenRuntime = model.runtime
                frozenHealth = model.health
                model.refreshStatisticsIfNeeded()
                return
            }
            frozenRuntime = nil
            frozenHealth = nil
            model.refreshStatisticsIfNeeded(force: true)
        }
        .onDisappear { model.setStatisticsVisible(false) }
        .onChange(of: selectedBoard) { _, board in
            model.setStatisticsBoard(board.rawValue)
        }
        .onExitCommand {
            // Escape exits the deepest active drill-down first.  The native
            // Table selection and the local filter stay in sync through the
            // same model-backed binding used by the project/session rows.
            if !model.runtimeLocalSessionID.isEmpty {
                model.clearRuntimeLocalSession()
            } else if !model.runtimeLocalProjectID.isEmpty || !model.runtimeLocalProjectName.isEmpty {
                model.clearRuntimeLocalProject()
            }
        }
        .sheet(isPresented: $storageSettingsPresented) {
            storageSettingsSheet
        }
        .sheet(isPresented: $cleanupPresented) {
            RuntimeCleanupSheet(model: model)
        }
        .sheet(isPresented: $pricingSheetPresented) {
            RuntimePricingEditor(model: model)
        }
        .confirmationDialog(
            "删除会话？",
            isPresented: deleteSessionDialogPresented,
            titleVisibility: .visible
        ) {
            Button("删除会话", role: .destructive) {
                if let sessionID = confirmDeleteSessionID {
                    model.deleteRuntimeSession(sessionID: sessionID)
                }
                confirmDeleteSessionID = nil
            }
            Button("取消", role: .cancel) {}
        } message: {
            let name = sessionDisplayName(confirmDeleteSessionID ?? "")
            Text("将删除会话 \(name) 的客户端请求、上游重试和对应统计；诊断捕获不会删除。")
        }
        .alert(
            "删除未识别会话？",
            isPresented: unidentifiedSessionAlertPresented,
            presenting: confirmUnidentifiedSessionID
        ) { sessionID in
            TextField("输入 DELETE 以确认", text: $unidentifiedConfirmationPhrase)
                .textContentType(.oneTimeCode)
            Button("确认删除", role: .destructive) {
                guard unidentifiedConfirmationPhrase == "DELETE" else {
                    model.flash("请输入 DELETE 后才能删除未识别会话")
                    return
                }
                model.deleteRuntimeSession(sessionID: sessionID, confirmUnidentified: true)
                confirmUnidentifiedSessionID = nil
                unidentifiedConfirmationPhrase = ""
            }
            .disabled(unidentifiedConfirmationPhrase != "DELETE")
            Button("取消", role: .cancel) {}
        } message: { _ in
            Text("未识别会话可能包含多个来源。删除会同时移除其请求、重试和 Token 统计；请输入 DELETE 才能继续。")
        }
        .confirmationDialog(
            "导出源运行字段？",
            isPresented: $confirmStoredExport,
            titleVisibility: .visible
        ) {
            Button("确认导出源数据", role: .destructive) {
                model.exportRuntimeAnalytics(scope: exportScope, format: exportFormat, privacy: "stored", confirmStored: true)
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("这不是完整诊断捕获，只包含 SQLite 已保存的源事件与会话归属字段；可能包含原始标识。导出前请确认用途和保存位置。")
        }
        .confirmationDialog(
            "清除会话粘性归属？",
            isPresented: Binding(
                get: { confirmStickyClearTarget != nil },
                set: { if !$0 { confirmStickyClearTarget = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("清除粘性归属", role: .destructive) {
                if let target = confirmStickyClearTarget {
                    stickyClearingKey = target.key
                    Task { @MainActor in
                        defer { stickyClearingKey = nil }
                        await model.clearRuntimeSticky(key: target.key, isSession: target.isSession)
                    }
                }
                confirmStickyClearTarget = nil
            }
            Button("取消", role: .cancel) {}
        } message: {
            let name = confirmStickyClearTarget?.name ?? ""
            let scope = confirmStickyClearTarget?.isSession == true ? "会话" : "项目"
            Text("清除\(scope)「\(name)」的粘性归属后，后续新请求会按当前路由规则重新选择入口。统计与事件不会被删除。")
        }
        .confirmationDialog(
            "估算源运行字段？",
            isPresented: $confirmStoredEstimate,
            titleVisibility: .visible
        ) {
            Button("确认估算源数据") {
                model.estimateRuntimeExport(scope: exportScope, format: exportFormat, privacy: "stored", confirmStored: true)
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("估算会按 SQLite 已保存的源字段计算行数和大小，可能暴露敏感标识的规模信息；不会包含完整诊断捕获。")
        }
        .onChange(of: exportScope) { _, _ in model.runtimeExportEstimate = nil }
        .onChange(of: exportFormat) { _, _ in model.runtimeExportEstimate = nil }
        .onChange(of: exportPrivacy) { _, _ in model.runtimeExportEstimate = nil }
    }

    /// One compact control surface for the statistics page.  Range, filters,
    /// snapshot state, reset and export belong to the same control layer;
    /// separating them into several full-width panels made the primary
    /// project comparison feel buried beneath implementation details.
    private var analyticsControlsPanel: some View {
        SectionPanel(
            title: "范围与筛选",
            hint: "所有条件均可选；留空时查看全部，选择后下面的指标和列表会一起更新。"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 12) {
                        rangePicker
                            .pickerStyle(.segmented)
                            .frame(maxWidth: 420)
                        Spacer(minLength: 0)
                        storageSummary
                        snapshotSummary
                        statisticsActionButtons
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        compactRangePicker
                        HStack(spacing: 10) {
                            storageSummary
                            snapshotSummary
                            Spacer(minLength: 0)
                            statisticsActionButtons
                        }
                    }
                }
                Divider()
                HStack {
                    Text("筛选条件").font(.subheadline.weight(.semibold))
                    Spacer()
                    clearAnalyticsFiltersButton
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 180), spacing: 12)], spacing: 10) {
                    analyticsFilterPickers
                }
                DisclosureGroup(isExpanded: $advancedFiltersExpanded) {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 180), spacing: 12)], spacing: 10) {
                        advancedAnalyticsFilterPickers
                    }.padding(.top, 8)
                } label: {
                    Text(advancedRuntimeFilterLabels.isEmpty ? "高级筛选 · 代理、用途与失败诊断" : "高级筛选 · 已选 \(advancedRuntimeFilterLabels.count) 项")
                        .font(.callout)
                }
                analyticsFilterStatus
                Divider()
                boardPicker
                if exportExpanded {
                    Divider()
                    VStack(alignment: .leading, spacing: 8) {
                        Text("导出统计")
                            .font(.subheadline.weight(.semibold))
                        Text("按当前筛选导出 SQLite 运行统计字段，不包含诊断正文。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        exportControls
                        if let estimate = model.runtimeExportEstimate {
                            Text("估算 \(tokenNumber(estimate.rowCount)) 行 · \(ByteCountFormatter.string(fromByteCount: Int64(clamping: estimate.estimatedBytes), countStyle: .file))")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        if let error = model.runtimeExportEstimateError {
                            Label(error, systemImage: "exclamationmark.triangle.fill")
                                .font(.caption)
                                .foregroundStyle(.orange)
                                .textSelection(.enabled)
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var snapshotSummary: some View {
        HStack(spacing: 6) {
            Label("数据状态", systemImage: "camera.viewfinder")
                .font(.caption)
                .foregroundStyle(.secondary)
            if model.runtimeHistoryLoading || model.runtimeV2Loading {
                ProgressView().controlSize(.small)
                Text("更新中…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else if model.runtimeHistoryPage != nil {
                Text("已同步")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Text("等待数据")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .fixedSize(horizontal: true, vertical: false)
        .help("统计数据来自同一份 SQLite 稳定快照")
    }

    @ViewBuilder
    private var statisticsActionButtons: some View {
        HStack(spacing: 6) {
            Button {
                exportExpanded.toggle()
            } label: {
                Label(exportExpanded ? "收起导出" : "导出", systemImage: "arrow.down.doc")
            }
            .controlSize(.small)
        }
        .fixedSize(horizontal: true, vertical: false)
    }

    @ViewBuilder
    private var analyticsFilterPickers: some View {
        Picker("入口", selection: Binding(
            get: { model.runtimeAnalyticsEndpointID },
            set: { model.setRuntimeAnalyticsFilters(endpointID: $0) }
        )) {
            Text("全部").tag("")
            ForEach(facets?.endpoints ?? []) { item in
                Text("\(model.config.endpoints.first(where: { $0.id == item.value })?.name ?? item.value) · \(item.count)")
                    .tag(item.value)
            }
        }
        Picker("项目", selection: Binding(
            get: { model.runtimeAnalyticsProject },
            set: { model.setRuntimeAnalyticsFilters(project: $0, sessionID: "") }
        )) {
            Text("全部").tag("")
            ForEach(facets?.projects ?? []) { item in
                Text("\(projectDisplayName(item.value)) · \(item.count)").tag(item.value)
            }
        }
        Picker("会话", selection: Binding(
            get: { model.runtimeAnalyticsSessionID },
            set: { model.setRuntimeAnalyticsFilters(sessionID: $0) }
        )) {
            Text("全部").tag("")
            ForEach(facets?.sessions ?? []) { item in
                Text("\(sessionDisplayName(item.value)) · \(item.count)").tag(item.value)
            }
        }
        Picker("客户端", selection: Binding(
            get: { model.runtimeAnalyticsClientKind },
            set: { model.setRuntimeAnalyticsFilters(clientKind: $0) }
        )) {
            Text("全部").tag("")
            ForEach(facets?.clientKinds ?? []) { item in
                Text("\(clientDisplayName(item.value)) · \(item.count)").tag(item.value)
            }
        }
        Picker("模型", selection: Binding(
            get: { model.runtimeAnalyticsModel },
            set: { model.setRuntimeAnalyticsFilters(model: $0) }
        )) {
            Text("全部").tag("")
            ForEach(facets?.models ?? []) { item in
                Text("\(item.value) · \(item.count)").tag(item.value)
            }
        }
        Picker("最终结果", selection: Binding(
            get: { model.runtimeAnalyticsOutcome },
            set: { model.setRuntimeAnalyticsFilters(outcome: $0) }
        )) {
            Text("全部").tag("")
            Text("成功").tag("succeeded")
            Text("失败").tag("failed")
            Text("已取消").tag("cancelled")
        }
    }

    @ViewBuilder
    private var advancedAnalyticsFilterPickers: some View {
        AttributionFacetPicker(title: "请求类型", selection: Binding(get: { model.runtimeAnalyticsRequestKind }, set: { model.setRuntimeAnalyticsFilters(requestKind: $0) }), rows: facets?.requestKinds ?? [])
        AttributionFacetPicker(title: "客户端形态", selection: Binding(get: { model.runtimeAnalyticsClientVariant }, set: { model.setRuntimeAnalyticsFilters(clientVariant: $0) }), rows: facets?.clientVariants ?? [])
        AttributionFacetPicker(title: "代理角色", selection: Binding(get: { model.runtimeAnalyticsAgentRole }, set: { model.setRuntimeAnalyticsFilters(agentRole: $0) }), rows: facets?.agentRoles ?? [])
        AttributionFacetPicker(title: "代理名称", selection: Binding(get: { model.runtimeAnalyticsAgentName }, set: { model.setRuntimeAnalyticsFilters(agentName: $0) }), rows: facets?.agentNames ?? [])
        AttributionFacetPicker(title: "父线程", selection: Binding(get: { model.runtimeAnalyticsParentThreadID }, set: { model.setRuntimeAnalyticsFilters(parentThreadID: $0) }), rows: facets?.parentThreads ?? [])
        AttributionFacetPicker(title: "父回合", selection: Binding(get: { model.runtimeAnalyticsParentTurnID }, set: { model.setRuntimeAnalyticsFilters(parentTurnID: $0) }), rows: facets?.parentTurns ?? [])
        AttributionFacetPicker(title: "根回合", selection: Binding(get: { model.runtimeAnalyticsRootTurnID }, set: { model.setRuntimeAnalyticsFilters(rootTurnID: $0) }), rows: facets?.rootTurns ?? [])
        Picker("用途", selection: Binding(
            get: { model.runtimeAnalyticsRequestPurpose },
            set: { model.setRuntimeAnalyticsFilters(requestPurpose: $0) }
        )) {
            Text("全部").tag("")
            ForEach(facets?.requestPurposes ?? []) { item in
                Text("\(dimensionValueDisplayName(item.value, kind: "purpose")) · \(item.count)").tag(item.value)
            }
        }
        Picker("失败类型", selection: Binding(
            get: { model.runtimeAnalyticsFailureKind },
            set: { model.setRuntimeAnalyticsFilters(failureKind: $0) }
        )) {
            Text("全部").tag("")
            ForEach(facets?.failureKinds ?? []) { item in
                Text("\(dimensionValueDisplayName(item.value, kind: "failure_kind")) · \(item.count)").tag(item.value)
            }
        }
        Picker("失败阶段", selection: Binding(
            get: { model.runtimeAnalyticsFailurePhase },
            set: { model.setRuntimeAnalyticsFilters(failurePhase: $0) }
        )) {
            Text("全部").tag("")
            ForEach(facets?.failurePhases ?? []) { item in
                Text("\(dimensionValueDisplayName(item.value, kind: "failure_phase")) · \(item.count)").tag(item.value)
            }
        }
    }

    private var clearAnalyticsFiltersButton: some View {
        Button("清除筛选") {
            model.setRuntimeAnalyticsFilters(requestKind: "", clientKind: "", clientVariant: "", agentRole: "", agentName: "", parentThreadID: "", parentTurnID: "", rootTurnID: "", endpointID: "", project: "", sessionID: "", model: "", requestPurpose: "", outcome: "", failureKind: "", failurePhase: "")
        }
        .disabled(!hasActiveRuntimeFilters)
    }

    private var boardPicker: some View {
        // Keep all five boards visible. A hidden menu made the statistics
        // hierarchy harder to scan precisely when the window became narrow.
        Group {
            if dynamicTypeSize.isAccessibilitySize {
                boardWrappedPicker
            } else {
                ViewThatFits(in: .horizontal) {
                    boardSegmentedPicker
                    boardWrappedPicker
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("统计看板")
    }

    private var boardSegmentedPicker: some View {
        Picker("看板", selection: $selectedBoard) {
            ForEach(StatisticsBoard.allCases) { board in
                Label(board.title, systemImage: board.systemImage).tag(board)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: .infinity, minHeight: 44)
    }

    private var boardWrappedPicker: some View {
        SumpterWrappingLayout(horizontalSpacing: 8, verticalSpacing: 8) {
            ForEach(StatisticsBoard.allCases) { board in
                Button {
                    selectedBoard = board
                } label: {
                    Label(board.title, systemImage: board.systemImage)
                        .font(.callout.weight(board == selectedBoard ? .semibold : .regular))
                        .frame(minHeight: 32)
                }
                .buttonStyle(.bordered)
                .tint(board == selectedBoard ? .accentColor : .secondary)
                .accessibilityValue(board == selectedBoard ? "已选择" : "")
            }
        }
    }

    private var compactRangePicker: some View {
        SumpterWrappingLayout(horizontalSpacing: 8, verticalSpacing: 8) {
            ForEach(["today", "7d", "30d", "all"], id: \.self) { value in
                Button {
                    model.setRuntimeAnalyticsRange(value)
                } label: {
                    Text(runtimeRangeLabel(value))
                        .font(.callout.weight(model.runtimeAnalyticsRange == value ? .semibold : .regular))
                        .frame(minHeight: 32)
                }
                .buttonStyle(.bordered)
                .tint(model.runtimeAnalyticsRange == value ? .accentColor : .secondary)
                .accessibilityValue(model.runtimeAnalyticsRange == value ? "已选择" : "")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("统计时间范围")
    }

    private func runtimeRangeLabel(_ value: String) -> String {
        switch value {
        case "today": "今天"
        case "7d": "7 天"
        case "30d": "30 天"
        default: "全部"
        }
    }

    @ViewBuilder
    private var selectedBoardContent: some View {
        switch selectedBoard {
        case .overview:
            overviewBoard
        case .trends:
            v3TrendSummary
        case .tokens:
            costBoard
        case .errors:
            VStack(alignment: .leading, spacing: 16) {
                v3ErrorTable
                diagnosticPanel
            }
        }
    }

    private var overviewBoard: some View {
        SectionPanel(
            title: "使用概览",
            hint: "按顺序查看核心指标、Token 与缓存、数据存储，以及入口、项目、会话和模型使用情况。项目或会话行可钻取模型明细。"
        ) {
            VStack(alignment: .leading, spacing: 14) {
                healthPanel
                Divider()
                overviewRequestSummary
                if let trend = model.runtimeTrendSeries, !trend.points.isEmpty {
                    Divider()
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Text("趋势脉搏快览").font(.subheadline.weight(.semibold))
                            Spacer()
                            Picker("指标", selection: $trendMetric) {
                                ForEach(TrendMetric.allCases) { metric in
                                    Text(metric.title).tag(metric)
                                }
                            }
                            .pickerStyle(.menu)
                            .frame(maxWidth: 200)
                        }
                        trendAreaChart(points: trend.points)
                    }
                }
                Divider()
                overviewTokenSummary
                Divider()
                storageOverviewPanel
                Divider()
                drilldownContextBar
                overviewDimensionTables
            }
        }
    }

    @ViewBuilder
    private var drilldownContextBar: some View {
        if !model.runtimeLocalProjectName.isEmpty || !model.runtimeLocalSessionName.isEmpty {
            HStack(spacing: 8) {
                Image(systemName: "arrow.turn.down.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(palette.brand)
                Text("全部维度")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                if !model.runtimeLocalProjectName.isEmpty {
                    Text("/")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                    HStack(spacing: 4) {
                        Text("项目")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                        Text(projectDisplayName(model.runtimeLocalProjectName))
                            .font(.caption.weight(.semibold))
                        Button {
                            model.clearRuntimeLocalProject()
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .help("清除项目下钻")
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(palette.brand.opacity(0.12), in: Capsule())
                }
                if !model.runtimeLocalSessionName.isEmpty {
                    Text("/")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                    HStack(spacing: 4) {
                        Text("会话")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                        Text(sessionDisplayName(model.runtimeLocalSessionName))
                            .font(.caption.weight(.semibold))
                        Button {
                            model.clearRuntimeLocalSession()
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .help("清除会话下钻")
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(palette.info.opacity(0.12), in: Capsule())
                }
                Spacer(minLength: 8)
                if !model.runtimeLocalSessionName.isEmpty {
                    Button("返回全部会话") {
                        model.clearRuntimeLocalSession()
                    }
                    .controlSize(.small)
                }
                if !model.runtimeLocalProjectName.isEmpty {
                    Button("重置下钻") {
                        model.clearRuntimeLocalProject()
                    }
                    .controlSize(.small)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(palette.inset, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .stroke(palette.brand.opacity(0.32), lineWidth: 0.9)
            )
        }
    }

    /// 四张首屏对比表始终同时出现。项目或会话行可作为本地钻取范围，
    /// 让模型表显示该范围内的分别用量。
    @ViewBuilder
    private var overviewDimensionTables: some View {
        VStack(alignment: .leading, spacing: 16) {
            runtimeOverviewDimensionTable(kind: "endpoint", title: "入口使用情况", page: model.runtimeEndpointsPage, sortOrder: $runtimeEndpointTableSort)
            runtimeOverviewDimensionTable(kind: "project", title: "项目使用情况", page: model.runtimeProjectsPage, sortOrder: $runtimeProjectTableSort)
            runtimeOverviewDimensionTable(kind: "session", title: "会话使用情况", page: model.runtimeSessionsPage, sortOrder: $runtimeSessionTableSort)
            runtimeOverviewDimensionTable(kind: "model", title: "模型使用情况", page: model.runtimeModelsPage, sortOrder: $runtimeModelTableSort)
        }
    }

    private enum RuntimeDimensionTableMode { case usage, cost }

    @ViewBuilder
    private func runtimeOverviewDimensionTable(
        kind: String,
        title: String,
        page: AdminWire.RuntimeDimensionPage?,
        sortOrder: Binding<[KeyPathComparator<AdminWire.RuntimeDimensionRow>]>,
        mode: RuntimeDimensionTableMode = .usage
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) {
                    runtimeOverviewDimensionHeading(kind: kind, title: title, page: page)
                    Spacer(minLength: 0)
                    runtimeOverviewDimensionControls(kind: kind, page: page)
                }
                VStack(alignment: .leading, spacing: 8) {
                    runtimeOverviewDimensionHeading(kind: kind, title: title, page: page)
                    runtimeOverviewDimensionControls(kind: kind, page: page)
                }
            }
            if kind == "project" {
                v3AttributionHints(page?.rows ?? [])
                piAttributionStatus(page?.rows ?? [])
            }
            if let page {
                if page.rows.isEmpty {
                    EmptyStateView(title: "暂无匹配的" + runtimeDimensionLabel(kind), systemImage: "chart.bar")
                } else {
                    if mode == .cost {
                        runtimeOverviewCostTable(page, kind: kind, sortOrder: sortOrder)
                    } else {
                        runtimeOverviewUsageTable(page, kind: kind, sortOrder: sortOrder)
                    }
                }
            } else if model.runtimeDimensionsLoading || model.runtimeV2Loading {
                HStack { ProgressView().controlSize(.small); Text("正在读取" + title + "…").font(.caption).foregroundStyle(.secondary) }
            } else {
                EmptyStateView(title: "暂无" + runtimeDimensionLabel(kind) + "使用数据", systemImage: "chart.bar")
            }
        }
    }

    @ViewBuilder
    private func v3AttributionHints(_ rows: [AdminWire.RuntimeDimensionRow]) -> some View {
        let hintRows = rows.map {
            ClaudeAttributionHint.ProjectRow(
                name: $0.name,
                projectSource: $0.source,
                clientKinds: $0.clientKinds ?? [],
                attempts: $0.requests
            )
        }
        if ClaudeAttributionHint.shouldPrompt(projects: hintRows) {
            attributionHintCard(
                title: "Claude Code 项目归因未配置",
                message: ClaudeAttributionHint.message,
                command: ClaudeAttributionHint.command
            )
        }
        if GrokAttributionHint.shouldPrompt(projects: hintRows) {
            attributionHintCard(
                title: "Grok Build 项目归因未配置",
                message: GrokAttributionHint.message,
                command: GrokAttributionHint.command
            )
        }
    }

    private func piAttributionStatus(_ rows: [AdminWire.RuntimeDimensionRow]) -> some View {
        let state = PiAttributionHint.state(projects: rows.map {
            .init(name: $0.name, projectSource: $0.source, clientKinds: $0.clientKinds ?? [], attempts: $0.requests)
        })
        return VStack(alignment: .leading, spacing: 6) {
            Text("pi 项目归因：" + state.message).font(.caption).foregroundStyle(.secondary)
            Link("pi 接入说明", destination: URL(string: "https://github.com/domoxiaojun/sumpter/blob/main/USAGE.md#pi-客户端")!)
                .font(.caption)
        }
    }

    private func runtimeOverviewUsageTable(_ page: AdminWire.RuntimeDimensionPage, kind: String, sortOrder: Binding<[KeyPathComparator<AdminWire.RuntimeDimensionRow>]>) -> some View {
        return Table(page.rows, selection: runtimeOverviewTableSelection(page: page, kind: kind), sortOrder: sortOrder) {
            TableColumn(runtimeDimensionLabel(kind), value: \.name) { row in runtimeOverviewDimensionName(row, kind: kind) }.width(min: 180, ideal: 240)
            TableColumn("请求", value: \.requests) { row in
                Text(tokenNumber(row.requests)).monospacedDigit()
            }.width(min: 84, ideal: 98)
            TableColumn("成功率", value: \.successRateForSorting) { row in Text(rateText(row.successRateForSorting >= 0 ? row.successRateForSorting : nil)).monospacedDigit() }.width(min: 92, ideal: 106)
            TableColumn("失败", value: \.failures) { row in Text(tokenNumber(row.failures)).monospacedDigit() }.width(min: 76, ideal: 88)
            TableColumn("输入 Token", value: \.inputTokens) { row in
                Text(dimensionTokenText(row.inputTokens, related: row.requests)).monospacedDigit()
            }.width(min: 112, ideal: 130)
            TableColumn("输出 Token", value: \.outputTokens) { row in Text(dimensionTokenText(row.outputTokens, related: row.requests)).monospacedDigit() }.width(min: 108, ideal: 126)
            TableColumn("输出速度", value: \.outputSpeedForSorting) { row in Text(RuntimeEventPresentation.outputSpeedDisplay(row.outputTokensPerSecond)).monospacedDigit() }.width(min: 108, ideal: 126)
            TableColumn("缓存读取", value: \.cacheReadInputTokens) { row in
                VStack(alignment: .leading, spacing: 2) {
                    Text(dimensionTokenText(row.cacheReadInputTokens, related: row.requests)).monospacedDigit()
                    Text("Token 读占比 " + rateText(dimensionCacheTokenRate(row))).font(.caption2.monospacedDigit()).foregroundStyle(.secondary)
                }
            }.width(min: 114, ideal: 136)
            TableColumn("平均耗时", value: \.averageDurationForSorting) { row in Text(row.averageDurationMS.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) } ?? "—").monospacedDigit() }.width(min: 108, ideal: 126)
            TableColumn("最近活动", value: \.lastSeen) { row in Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: row.lastSeen))).font(.caption.monospacedDigit()) }.width(min: 150, ideal: 174)
        }
        .onChange(of: sortOrder.wrappedValue) { _, order in applyRuntimeOverviewSort(order, kind: kind) }
        .sumpterTableSurface()
        .frame(maxWidth: .infinity)
        .frame(height: min(280, max(96, CGFloat(page.rows.count) * 31 + 38)))
    }

    private func runtimeOverviewCostTable(_ page: AdminWire.RuntimeDimensionPage, kind: String, sortOrder: Binding<[KeyPathComparator<AdminWire.RuntimeDimensionRow>]>) -> some View {
        return Table(page.rows, selection: runtimeOverviewTableSelection(page: page, kind: kind), sortOrder: sortOrder) {
            TableColumn(runtimeDimensionLabel(kind), value: \.name) { row in runtimeOverviewDimensionName(row, kind: kind) }.width(min: 180, ideal: 240)
            // Cost is calculated after the server applies its supported
            // aggregate ordering; keep it read-only here instead of sorting
            // only the visible page on the client.
            TableColumn("成本") { row in Text(costText(row.cost?.estimatedCostMicros ?? 0, currency: row.cost?.currency)).monospacedDigit() }.width(min: 110, ideal: 130)
            TableColumn("请求", value: \.requests) { row in
                Text(tokenNumber(row.requests)).monospacedDigit()
            }.width(min: 84, ideal: 98)
            TableColumn("暂无法计价") { row in Text(tokenNumber((row.cost?.unpricedRequests ?? 0) + (row.cost?.unknownAccountingRequests ?? 0))).monospacedDigit() }.width(min: 110, ideal: 126)
            TableColumn("最近活动", value: \.lastSeen) { row in Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: row.lastSeen))).font(.caption.monospacedDigit()) }.width(min: 150, ideal: 174)
        }
        .onChange(of: sortOrder.wrappedValue) { _, order in applyRuntimeOverviewSort(order, kind: kind) }
        .sumpterTableSurface()
        .frame(maxWidth: .infinity)
        .frame(height: min(240, max(96, CGFloat(page.rows.count) * 31 + 38)))
    }

    private func runtimeOverviewDimensionHeading(kind: String, title: String, page: AdminWire.RuntimeDimensionPage?) -> some View {
        HStack(spacing: 8) {
            Text(runtimeOverviewDimensionTitle(kind: kind, title: title))
                .font(.subheadline.weight(.semibold))
            if let page { Text("共 " + tokenNumber(page.totalCount)).font(.caption.monospacedDigit()).foregroundStyle(.secondary) }
            if kind == "session", !model.runtimeLocalProjectID.isEmpty {
                Button("清除项目选择") { model.clearRuntimeLocalProject() }.controlSize(.small)
            }
            if kind == "model", !model.runtimeLocalSessionName.isEmpty {
                Button("清除会话选择") { model.clearRuntimeLocalSession() }.controlSize(.small)
            } else if kind == "model", !model.runtimeLocalProjectName.isEmpty {
                Button("清除项目选择") { model.clearRuntimeLocalProject() }.controlSize(.small)
            }
        }
    }

    private func runtimeOverviewDimensionTitle(kind: String, title: String) -> String {
        if kind == "model", !model.runtimeLocalSessionName.isEmpty {
            return title + " · 会话：" + sessionDisplayName(model.runtimeLocalSessionName)
        }
        if kind == "model", !model.runtimeLocalProjectName.isEmpty {
            return title + " · 项目：" + projectDisplayName(model.runtimeLocalProjectName)
        }
        if kind == "session", !model.runtimeLocalProjectName.isEmpty {
            return title + " · 项目：" + projectDisplayName(model.runtimeLocalProjectName)
        }
        return title
    }

    @ViewBuilder
    private func runtimeOverviewDimensionControls(kind: String, page: AdminWire.RuntimeDimensionPage?) -> some View {
        let loading = model.runtimeDimensionsLoading
        HStack(spacing: 6) {
            TextField("搜索" + runtimeDimensionLabel(kind), text: Binding(
                get: {
                    switch kind { case "endpoint": model.runtimeEndpointSearch; case "project": model.runtimeProjectSearch; case "session": model.runtimeSessionSearch; default: model.runtimeModelSearch }
                },
                set: {
                    switch kind { case "endpoint": model.runtimeEndpointSearch = String($0.prefix(256)); case "project": model.runtimeProjectSearch = String($0.prefix(256)); case "session": model.runtimeSessionSearch = String($0.prefix(256)); default: model.runtimeModelSearch = String($0.prefix(256)) }
                }
            ))
            .textFieldStyle(.roundedBorder)
            .frame(minWidth: 130, idealWidth: 180)
            .onSubmit { loadRuntimeOverviewPage(kind: kind, page: 1) }
            Button("搜索") { loadRuntimeOverviewPage(kind: kind, page: 1) }.controlSize(.small)
            if let page {
                SumpterPaginationControls(
                    page: page.page,
                    totalPages: page.totalPages,
                    hasPrevious: page.hasPrevious,
                    hasNext: page.hasNext,
                    pageSize: runtimeOverviewPageSize(kind),
                    compact: true,
                    loading: loading,
                    onPageChange: { loadRuntimeOverviewPage(kind: kind, page: $0) },
                    onPageSizeChange: { setRuntimeOverviewPageSize(kind: kind, size: $0) }
                )
            }
        }
    }

    private func runtimeOverviewPageSize(_ kind: String) -> Int {
        switch kind { case "endpoint": model.runtimeEndpointPageSize; case "project": model.runtimeProjectPageSize; case "session": model.runtimeSessionPageSize; default: model.runtimeModelPageSize }
    }

    private func loadRuntimeOverviewPage(kind: String, page: Int) {
        switch kind { case "endpoint": model.loadRuntimeEndpoints(page: page); case "project": model.loadRuntimeProjects(page: page); case "session": model.loadRuntimeSessions(page: page); default: model.loadRuntimeModels(page: page) }
    }

    private func setRuntimeOverviewPageSize(kind: String, size: Int) {
        switch kind { case "endpoint": model.setRuntimeEndpointPageSize(size); case "project": model.setRuntimeProjectPageSize(size); case "session": model.setRuntimeSessionPageSize(size); default: model.setRuntimeModelPageSize(size) }
    }

    /// Native Table selection makes the entire row a hit target and supplies
    /// the standard macOS full-row highlight.  Keep the selection source in
    /// AppModel so the table, the drill-down title, and the loaded data cannot
    /// diverge when a row is selected by keyboard or by clicking blank cells.
    private func runtimeOverviewTableSelection(
        page: AdminWire.RuntimeDimensionPage,
        kind: String
    ) -> Binding<String?> {
        let isSelectable = kind == "project" || kind == "session"
        return Binding(
            get: {
                guard isSelectable else { return nil }
                let selectedKey = kind == "project" ? model.runtimeLocalProjectID : model.runtimeLocalSessionID
                guard !selectedKey.isEmpty else { return nil }
                return page.rows.first(where: { $0.key == selectedKey })?.id
            },
            set: { selectedID in
                guard isSelectable else { return }
                guard let selectedID else {
                    if kind == "project" {
                        model.clearRuntimeLocalProject()
                    } else {
                        model.clearRuntimeLocalSession()
                    }
                    return
                }
                guard let row = page.rows.first(where: { $0.id == selectedID }) else { return }
                if kind == "project" {
                    model.setRuntimeLocalProject(row.key, projectName: row.name)
                } else {
                    model.setRuntimeLocalSession(row.key, sessionName: row.name)
                }
            }
        )
    }

    @ViewBuilder
    private func runtimeOverviewDimensionName(_ row: AdminWire.RuntimeDimensionRow, kind: String) -> some View {
        let subtitle: String = if kind == "project" {
            "\(projectClientKindsLabel(row.clientKinds)) · \(dimensionSourceDisplayName(row.source, kind: kind))"
        } else {
            dimensionSourceDisplayName(row.source, kind: kind)
        }
        let content = VStack(alignment: .leading, spacing: 2) {
            Text(runtimeDimensionDisplayName(row.name, kind: kind)).lineLimit(1).help(row.key)
            Text(subtitle).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
        }
        if kind == "project" {
            HStack(spacing: 6) {
                Button { model.setRuntimeLocalProject(row.key, projectName: row.name) } label: { content }
                    .buttonStyle(.link)
                    .help("仅筛选下方会话：\(row.key)")
                    .accessibilityLabel("仅查看项目 \(runtimeDimensionDisplayName(row.name, kind: kind)) 的会话和模型用量")
                Spacer(minLength: 4)
                if overviewProjectCanLocate(row) {
                    Button {
                        locateOverviewProject(row)
                    } label: {
                        if locatingProjectName == row.name {
                            ProgressView().controlSize(.mini)
                        } else {
                            Image(systemName: "folder")
                                .font(.caption)
                        }
                    }
                    .buttonStyle(.borderless)
                    .disabled(locatingProjectName != nil)
                    .help("在 Finder 中定位项目本地路径")
                    .accessibilityLabel("在 Finder 中定位项目 \(runtimeDimensionDisplayName(row.name, kind: kind))")
                }
                Button {
                    confirmStickyClearTarget = (key: row.key, name: runtimeDimensionDisplayName(row.name, kind: kind), isSession: false)
                } label: {
                    Image(systemName: stickyClearingKey == row.key ? "hourglass" : "arrow.triangle.2.circlepath")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .disabled(stickyClearingKey != nil)
                .help("清除该项目的会话粘性归属：新请求按当前路由规则重新选择入口")
                .accessibilityLabel("清除项目 \(runtimeDimensionDisplayName(row.name, kind: kind)) 的粘性归属")
            }
        } else if kind == "session" {
            HStack(spacing: 6) {
                Button { model.setRuntimeLocalSession(row.key, sessionName: row.name) } label: { content }
                    .buttonStyle(.link)
                    .help("仅查看会话：\(row.key) 的模型用量")
                    .accessibilityLabel("仅查看会话 \(runtimeDimensionDisplayName(row.name, kind: kind)) 的模型用量")
                Spacer(minLength: 4)
                Button {
                    confirmStickyClearTarget = (key: row.key, name: runtimeDimensionDisplayName(row.name, kind: kind), isSession: true)
                } label: {
                    Image(systemName: stickyClearingKey == row.key ? "hourglass" : "arrow.triangle.2.circlepath")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .disabled(stickyClearingKey != nil || row.key == "unidentified_session")
                .help(row.key == "unidentified_session" ? "未识别会话包含多个对话，不能单独清除" : "只解除此对话的入口绑定，保留统计与事件")
                .accessibilityLabel("清除会话 \(runtimeDimensionDisplayName(row.name, kind: kind)) 的粘性归属")
                Button {
                    model.exportRuntimeSession(sessionID: row.name)
                } label: {
                    Image(systemName: "square.and.arrow.down")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .help("导出会话 JSON")
                .accessibilityLabel("导出会话 \(runtimeDimensionDisplayName(row.name, kind: kind))")
                Button(role: .destructive) {
                    if row.name == "unidentified_session" {
                        unidentifiedConfirmationPhrase = ""
                        confirmUnidentifiedSessionID = row.name
                    } else {
                        confirmDeleteSessionID = row.name
                    }
                } label: {
                    Image(systemName: "trash")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .help(row.name == "unidentified_session" ? "危险操作：删除未识别会话（需输入 DELETE）" : "删除会话及统计")
                .accessibilityLabel("删除会话 \(runtimeDimensionDisplayName(row.name, kind: kind))")
            }
        } else { content }
    }

    private func overviewProjectCanLocate(_ row: AdminWire.RuntimeDimensionRow) -> Bool {
        guard row.name != "unidentified_project", row.name != "multiple_workspaces" else {
            return false
        }
        switch row.source {
        case "workspace_remote_fallback", "missing_workspace_metadata", "workspace_unidentified":
            return false
        default:
            return true
        }
    }

    private func locateOverviewProject(_ row: AdminWire.RuntimeDimensionRow) {
        guard overviewProjectCanLocate(row), locatingProjectName == nil else { return }
        let projectName = row.name
        let workspacePaths = ProjectFinder.workspacePaths(projectName: projectName, events: projectFinderEvents)
        locatingProjectName = projectName
        Task { @MainActor in
            let outcome = await ProjectFinder.locate(
                projectName: projectName,
                workspacePaths: workspacePaths
            )
            guard locatingProjectName == projectName else { return }
            locatingProjectName = nil
            switch outcome {
            case .matches(let paths):
                let urls = paths.map { URL(fileURLWithPath: $0, isDirectory: true) }
                guard !urls.isEmpty else {
                    model.flash("未找到可验证的本地目录；本地路径已脱敏")
                    return
                }
                NSWorkspace.shared.activateFileViewerSelecting(urls)
                if urls.count == 1 {
                    model.flash("已在 Finder 中定位：\(projectDisplayName(projectName))")
                } else {
                    model.flash("Finder 已选中 \(urls.count) 个同名目录，请确认本地目录")
                }
            case .unavailable:
                model.flash("未找到可验证的本地目录；本地路径已脱敏")
            }
        }
    }

    private var costBoard: some View {
        SectionPanel(title: "成本", hint: "先看成本摘要，再比较入口、项目、会话和模型成本；项目或会话行可钻取模型明细。") {
            VStack(alignment: .leading, spacing: 14) {
                costSummary
                Divider()
                drilldownContextBar
                runtimeOverviewDimensionTable(kind: "endpoint", title: "入口成本", page: model.runtimeEndpointsPage, sortOrder: $runtimeEndpointTableSort, mode: .cost)
                runtimeOverviewDimensionTable(kind: "project", title: "项目成本", page: model.runtimeProjectsPage, sortOrder: $runtimeProjectTableSort, mode: .cost)
                runtimeOverviewDimensionTable(kind: "session", title: "会话成本", page: model.runtimeSessionsPage, sortOrder: $runtimeSessionTableSort, mode: .cost)
                runtimeOverviewDimensionTable(kind: "model", title: "模型成本", page: model.runtimeModelsPage, sortOrder: $runtimeModelTableSort, mode: .cost)
            }
        }
    }

    private var costSummary: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("成本摘要").font(.subheadline.weight(.semibold))
                Spacer()
                Button {
                    pricingSheetPresented = true
                } label: {
                    Label("配置模型价格…", systemImage: "tag")
                }
                .controlSize(.small)
            }
            if let total = model.runtimeTrendSeries?.totals {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 160), spacing: 10)], spacing: 10) {
                    MetricTile(title: "估算成本", value: costText(total.cost.estimatedCostMicros, currency: total.cost.currency), detail: total.cost.complete ? "已完成计价" : "部分请求暂无法计价", systemImage: "yensign.circle")
                    MetricTile(title: "已计价请求", value: tokenNumber(total.cost.pricedRequests), detail: "已匹配入口与模型价格", systemImage: "checkmark.circle")
                    MetricTile(title: "暂无法计价", value: tokenNumber(total.cost.unpricedRequests + total.cost.unknownAccountingRequests), detail: "缺少价格或完整用量 · 可在右上角补充模型单价", systemImage: "questionmark.circle")
                }
            } else {
                Text("暂无成本摘要").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var overviewRequestSummary: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("核心指标")
                .font(.subheadline.weight(.semibold))
            if let total = model.runtimeTrendSeries?.totals {
                let completed = total.clientSuccesses + total.clientFailures + total.clientCancelled
                let pending = max(0, total.clientRequests - completed)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                    MetricTile(title: "请求", value: tokenNumber(total.clientRequests), detail: "已完成 \(tokenNumber(completed)) · 待定 \(tokenNumber(pending))", systemImage: "arrow.left.arrow.right", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "成功率", value: completed > 0 ? String(format: "%.1f%%", Double(total.clientSuccesses) * 100 / Double(completed)) : "—", detail: "成功 \(tokenNumber(total.clientSuccesses)) · 失败 \(tokenNumber(total.clientFailures)) · 取消 \(tokenNumber(total.clientCancelled))", systemImage: "checkmark.circle", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "平均首字节", value: latencyAverage(total.ttfbMS), detail: "首次响应平均耗时", systemImage: "timer", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "平均完成耗时", value: latencyAverage(total.durationMS), detail: "请求完成平均耗时", systemImage: "clock", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "输出速度", value: RuntimeEventPresentation.outputSpeedDisplay(total.outputTokensPerSecond), detail: outputSpeedDetail(total.outputSpeedRequests), systemImage: "speedometer", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "故障转移", value: tokenNumber(total.failovers), detail: "恢复 \(tokenNumber(total.failoverRecoveredRequests)) · 最终失败 \(tokenNumber(total.failoverTerminalRequests))", systemImage: "arrow.triangle.2.circlepath", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "上游尝试", value: tokenNumber(total.upstreamAttempts), detail: "成功 \(tokenNumber(total.upstreamSuccesses)) · 失败 \(tokenNumber(total.upstreamFailures))", systemImage: "server.rack", minimumHeight: overviewMetricMinimumHeight)
                }
            } else if model.runtimeV2Loading {
                HStack { ProgressView().controlSize(.small); Text("正在读取核心指标…").font(.caption).foregroundStyle(.secondary) }
            } else {
                EmptyStateView(title: "暂无请求统计", systemImage: "chart.bar")
            }
        }
    }

    @ViewBuilder
    private var overviewTokenSummary: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Token 与缓存")
                .font(.subheadline.weight(.semibold))
            let usage = model.runtimeTrendSeries?.totals.tokens
            let cache = model.runtimeTrendSeries?.cacheRead
            HStack(spacing: 16) {
                Text("已确认缓存命中率 " + rateText(cache?.confirmedHitRate))
                Text("结论覆盖率 " + rateText(cache?.confirmationCoverage))
            }.font(.callout)
            if let cache {
                Text("未知 \(cache.unknownRequests) · 适用性不明 \(cache.applicabilityUnknownRequests) · 不适用 \(cache.notApplicableRequests)；命中率只统计已确认命中/未命中的已结束请求。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                MetricTile(title: "输入 Token", value: tokenText(usage?.inputTokens, presence: usage?.usageFieldPresence.inputTokens), detail: "输入用量", systemImage: "arrow.down.doc", minimumHeight: overviewMetricMinimumHeight)
                MetricTile(title: "输出 Token", value: tokenText(usage?.outputTokens, presence: usage?.usageFieldPresence.outputTokens), detail: "输出用量", systemImage: "arrow.up.doc", minimumHeight: overviewMetricMinimumHeight)
                MetricTile(title: "缓存读取", value: tokenText(usage?.cacheReadInputTokens, presence: usage?.usageFieldPresence.cacheReadInputTokens), detail: "缓存读取用量", footerAccessory: "Token 读占比 " + rateText(usage?.cacheReadTokenRate), systemImage: "externaldrive.badge.checkmark", minimumHeight: overviewMetricMinimumHeight)
                MetricTile(title: "缓存写入", value: tokenText(usage?.cacheCreationInputTokens, presence: usage?.usageFieldPresence.cacheCreationInputTokens), detail: "缓存写入 Token", systemImage: "externaldrive.badge.plus", minimumHeight: overviewMetricMinimumHeight)
            }
        }
    }

    @ViewBuilder
    private var storageOverviewPanel: some View {
        VStack(alignment: .leading, spacing: 12) {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    storageOverviewTitle
                    Spacer(minLength: 12)
                    storageOverviewAction
                }
                VStack(alignment: .leading, spacing: 8) {
                    storageOverviewTitle
                    storageOverviewAction
                }
            }
            Text("系统按滚动 24 小时的保存天数和 SQLite 有效占用上限自动轮换；任一条件先达到就触发。按请求组删除，进行中的请求组会完整保留。轮换不会立即缩小数据库文件；需要真正回收空间时请使用“重置并新建数据库”。仍可随时按时间清理已完成统计。")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let probe = model.runtimeStorageProbe {
                let live = model.runtimeSummary?.storage
                let pendingEvents = live?.pendingEvents ?? probe.pendingEvents
                let pendingBytes = live?.pendingBytes ?? probe.pendingBytes
                if probe.legacyRetentionDetected == true {
                    Label("检测到旧版本自动清理设置；它们已不再生效，也不会自动迁移。手动清理只删除事件，需使用“重置并新建数据库”移除旧字段。", systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(8)
                        .background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                    MetricTile(title: "运行状态", value: live?.state == "backpressure" ? "写入积压" : (live?.state == "degraded" ? "需要关注" : (probe.projectionIndexesReady && probe.projectionBackfillComplete ? "运行正常" : "处理中")), detail: live?.lastError ?? "SQLite 统计存储", systemImage: "externaldrive.fill", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "已保存事件", value: tokenNumber(probe.retainedEvents), detail: "完成 \(tokenNumber(probe.completedEvents)) · 进行中 \(tokenNumber(probe.inFlightEvents))", systemImage: "tray.full", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "有效占用", value: formatBytes(probe.liveBytes), detail: "文件 \(formatBytes(probe.databaseBytes)) · WAL \(formatBytes(probe.walBytes))", systemImage: "internaldrive", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "保留策略", value: retentionPolicyLabel(probe.retention), detail: retentionPolicyDetail(probe.retention), systemImage: "arrow.triangle.2.circlepath", minimumHeight: overviewMetricMinimumHeight)
                    MetricTile(title: "待写入", value: pendingEvents.map(tokenNumber) ?? "—", detail: pendingBytes.map(formatBytes) ?? (live == nil ? "暂未提供队列状态" : "队列正常"), systemImage: "arrow.down.doc", minimumHeight: overviewMetricMinimumHeight)
                }
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("正在读取数据存储状态…")
                }
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(14)
        .background(palette.raised, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(palette.brand.opacity(0.42), lineWidth: 1)
        )
    }

    private var storageOverviewTitle: some View {
        HStack(spacing: 8) {
            Image(systemName: "externaldrive.fill")
                .foregroundStyle(palette.brand)
            Text("运行统计存储")
                .font(.headline)
            if model.runtimeStorageProbe != nil {
                StatusBadge(
                    text: model.runtimeStorageProbe.map { retentionPolicyLabel($0.retention) } ?? "读取中",
                    systemImage: model.runtimeStorageProbe.map { retentionPolicyIsAutomatic($0.retention) ? "arrow.triangle.2.circlepath" : "hand.raised.fill" } ?? "hourglass",
                    color: palette.success
                )
            }
        }
    }

    private var storageOverviewAction: some View {
        HStack(spacing: 8) {
            Button {
                storageSettingsPresented = true
            } label: {
                Label("存储设置…", systemImage: "slider.horizontal.3")
            }
            .controlSize(.small)
            Button(role: .destructive) {
                cleanupPresented = true
            } label: {
                Label("按时间清理统计", systemImage: "trash")
            }
            .controlSize(.small)
            .help("按时间清理已完成 SQLite 运行统计；不会删除诊断捕获")
            RecreateDatabaseButton(model: model)
        }
    }

    @ViewBuilder
    private var storageSettingsSheet: some View {
        if let probe = model.runtimeStorageProbe {
            StorageLimitEditor(
                model: model,
                probe: probe,
                presentation: .sheet(onClose: { storageSettingsPresented = false })
            )
        } else {
            SheetShell(
                title: "运行统计存储设置",
                primaryTitle: "关闭",
                onCancel: { storageSettingsPresented = false },
                onSubmit: { storageSettingsPresented = false }
            ) {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("正在读取存储状态…")
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private func retentionPolicyIsAutomatic(_ retention: AdminWire.RuntimeRetention) -> Bool {
        retention.maxAgeDays != nil || retention.storageLimitBytes != nil
    }

    private func retentionPolicyLabel(_ retention: AdminWire.RuntimeRetention) -> String {
        retentionPolicyIsAutomatic(retention) ? "自动轮换" : "仅手动清理"
    }

    private func retentionPolicyDetail(_ retention: AdminWire.RuntimeRetention) -> String {
        switch (retention.maxAgeDays, retention.storageLimitBytes) {
        case let (age?, bytes?):
            return "最长 \(age) 天 · 容量 \(formatBytes(bytes))"
        case let (age?, nil):
            return "最长 \(age) 天 · 容量不限制"
        case let (nil, bytes?):
            return "时间不限制 · 容量 \(formatBytes(bytes))"
        case (nil, nil):
            return "未设置自动条件"
        }
    }

    @ViewBuilder
    private func attributionHintCard(title: String, message: String, command: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "info.circle")
                Text(title)
                    .font(.caption.weight(.semibold))
            }
            Text(message)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Text(command)
                    .font(.caption2.monospaced())
                    .textSelection(.enabled)
                Button("复制命令") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(command, forType: .string)
                }
                .controlSize(.small)
            }
            Text("配置器在仓库 platforms/macos/scripts/ 下（跨平台通用）。装完要新开一个终端才生效。")
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(8)
        .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
    }

    private func projectDisplayName(_ value: String) -> String {
        switch value {
        case "unidentified_project": "未识别项目"
        case "multiple_workspaces": "多工作区（未拆分）"
        default: value
        }
    }

    private var projectFinderEvents: [RuntimeEvent] {
        var seen = Set<String>()
        let paged = model.runtimePage?.events.map(\.runtimeEvent) ?? []
        return (displayRuntime.recentEvents + paged).filter { event in
            seen.insert(event.id).inserted
        }
    }

    private func clientDisplayName(_ value: String) -> String {
        if value == "unrecorded_client" { return "旧事件（未记录）" }
        return ClientKind(rawValue: value)?.displayName ?? value
    }

    private func runtimeOutcomeDisplayName(_ value: String) -> String {
        switch value {
        case "succeeded": "成功"
        case "failed": "失败"
        case "cancelled": "已取消"
        default: value.isEmpty ? "未记录" : value
        }
    }

    private func projectClientKindsLabel(_ kinds: [String]?) -> String {
        var labels: [String] = []
        for kind in kinds ?? [] {
            let label = kind == "unrecorded_client" ? "客户端未记录" : clientDisplayName(kind)
            guard !label.isEmpty, !labels.contains(label) else { continue }
            labels.append(label)
        }
        return labels.isEmpty ? "客户端未记录" : labels.joined(separator: " · ")
    }

    private func dimensionValueDisplayName(_ value: String, kind: String) -> String {
        switch kind {
        case "project": return projectDisplayName(value)
        case "session": return sessionDisplayName(value)
        case "client_kind": return clientDisplayName(value)
        case "request_kind": return RuntimeEventPresentation.observedRequestKind(value)
        case "client_variant": return RuntimeEventPresentation.clientVariant(value)
        case "agent_role": return RuntimeEventPresentation.agentRole(value)
        case "purpose":
            switch value {
            case "normal", "standard": return "主对话"
            case "session_title", "title": return "会话标题生成"
            case "websearch", "webSearch": return "WebSearch 搜索"
            case "webfetch", "webFetch": return "WebFetch 抓取"
            case "classifier": return "安全分类器"
            case "image_generation": return "图片生成"
            case "image_edit": return "图片编辑"
            case "alpha_search": return "Codex 独立搜索"
            case "token_count": return "Token 计数"
            case "compact": return "上下文压缩"
            default: return value.isEmpty ? "未记录" : value
            }
        case "failure_kind":
            let labels = [
                "response_timeout": "响应超时",
                "connection_failed": "连接失败",
                "invalid_response": "上游响应格式异常",
                "upstream_http_status": "上游返回 HTTP 错误状态",
                "stream_idle_timeout": "流式空闲超时",
                "stream_interrupted": "流传输中途断开",
                "upstream_response_incomplete": "上游响应流未完整结束",
                "upstream_response_failed": "上游响应协议失败",
                "endpoints_exhausted": "所有可用入口全部耗尽",
                "client_cancelled": "客户端断开或取消",
                "client_disconnected": "连接中断（原因未确定）",
                "response_wait_timeout": "代理响应等待超时",
                "client_request_rejected": "客户端请求被代理拒绝",
            ]
            return labels[value] ?? (value.isEmpty ? "未分类" : value)
        case "failure_phase":
            let labels = [
                "before_response": "建立连接 / 请求发送前",
                "response_headers": "接收响应头阶段",
                "response_stream": "流式传输输出阶段",
            ]
            return labels[value] ?? (value.isEmpty ? "未记录" : value)
        case "stream_terminal":
            let labels = [
                "completed": "正常结束",
                "failed": "失败结束",
                "incomplete": "未完整结束",
                "interrupted": "中途断开",
                "pending": "进行中",
            ]
            return labels[value] ?? (value.isEmpty ? "未记录" : value)
        case "protocol":
            let labels = [
                "anthropic_messages": "Anthropic Messages",
                "openai_chat": "OpenAI Chat Completions",
                "openai_responses": "OpenAI Responses",
            ]
            return labels[value] ?? (value.isEmpty ? "未记录" : value)
        default: return value.isEmpty ? "未记录" : value
        }
    }

    private func dimensionSourceDisplayName(_ value: String, kind: String) -> String {
        guard kind == "project" else { return value.isEmpty ? "—" : value }
        switch value {
        case "workspace_local": return "来源：本地项目"
        case "client_declared": return "来源：客户端声明"
        case "workspace_remote_fallback": return "来源：远程仓库回退识别"
        case "missing_workspace_metadata": return "来源：未记录"
        case "multiple_workspaces": return "来源：多工作区（未拆分）"
        case "workspace_unidentified": return "来源：未识别"
        case "mixed": return "来源：混合项目来源"
        default: return value.isEmpty ? "来源：未记录" : "来源：\(value)"
        }
    }

    private func sessionDisplayName(_ value: String) -> String {
        if value == "unidentified_session" { return "未识别会话" }
        guard value.count > 28 else { return value }
        return "\(value.prefix(12))…\(value.suffix(10))"
    }

    private func tokenText(_ value: Int?) -> String {
        RuntimeEventPresentation.tokenCountDisplay(value)
    }

    /// When the server provides field-presence counters, zero presence means
    /// the aggregate is not evidence of a real zero. Older daemons omit the
    /// counters, so their optional numeric value remains the compatibility path.
    private func tokenText(_ value: Int?, presence: Int?) -> String {
        guard let presence else { return tokenText(value) }
        guard presence > 0 else { return "—" }
        return tokenText(value)
    }

    /// Rates from the daemon are normalized to 0…1. Keep the display rule in
    /// one place so an explicit zero remains `0.0%`, while a missing rate stays
    /// visibly unknown instead of being mistaken for a measured zero.
    private func rateText(_ value: Double?) -> String {
        guard let value, value.isFinite else { return "—" }
        let percent = min(100, max(0, value * 100))
        return String(format: "%.1f%%", locale: Locale(identifier: "en_US_POSIX"), percent)
    }

    private func formatBytes(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .memory)
    }

    private var rangePicker: some View {
        Picker("范围", selection: Binding(
            get: { model.runtimeAnalyticsRange },
            set: { model.setRuntimeAnalyticsRange($0) }
        )) {
            Text("今天").tag("today")
            Text("7 天").tag("7d")
            Text("30 天").tag("30d")
            Text("全部").tag("all")
        }
    }

    @ViewBuilder
    private var storageSummary: some View {
        if let probe = model.runtimeStorageProbe {
            let live = model.runtimeSummary?.storage
            HStack(spacing: 8) {
                StatusBadge(
                    text: retentionPolicyLabel(probe.retention),
                    systemImage: retentionPolicyIsAutomatic(probe.retention) ? "arrow.triangle.2.circlepath" : "hand.raised.fill",
                    color: palette.success
                )
                Text("\(retentionPolicyDetail(probe.retention)) · \(tokenNumber(probe.retainedEvents)) 条 · 待写 \(live.map { tokenNumber($0.pendingEvents) } ?? probe.pendingEvents.map(tokenNumber) ?? "—")")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var v3TrendSummary: some View {
        SectionPanel(
            title: "趋势",
            hint: "使用上方范围筛选查看请求、响应速度、Token、缓存、故障转移与成本。"
        ) {
            if let trend = model.runtimeTrendSeries {
                VStack(alignment: .leading, spacing: 14) {
                    let total = trend.totals
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                        MetricTile(title: "请求", value: tokenNumber(total.clientRequests), detail: "快照内客户端请求", systemImage: "arrow.left.arrow.right")
                        MetricTile(title: "成功", value: tokenNumber(total.clientSuccesses), detail: "失败 \(tokenNumber(total.clientFailures)) · 取消 \(tokenNumber(total.clientCancelled))", systemImage: "checkmark.circle")
                        MetricTile(title: "故障转移", value: tokenNumber(total.failovers), detail: "恢复 \(tokenNumber(total.failoverRecoveredRequests)) · 最终失败 \(tokenNumber(total.failoverTerminalRequests))", systemImage: "arrow.triangle.2.circlepath")
                        MetricTile(title: "上游尝试", value: tokenNumber(total.upstreamAttempts), detail: "成功 \(tokenNumber(total.upstreamSuccesses)) · 失败 \(tokenNumber(total.upstreamFailures))", systemImage: "server.rack")
                        MetricTile(title: "慢请求", value: latencyThresholdPair(total.durationMS, thresholds: trend.thresholds.durationMS), detail: latencyDetail(total.durationMS, thresholds: trend.thresholds.durationMS), systemImage: "tortoise")
                        MetricTile(title: "首字节慢请求", value: latencyThresholdPair(total.ttfbMS, thresholds: trend.thresholds.ttfbMS), detail: latencyDetail(total.ttfbMS, thresholds: trend.thresholds.ttfbMS), systemImage: "timer")
                        MetricTile(title: "Token", value: tokenNumber(total.tokens.processedTotalTokens), detail: "输入/输出按协议去重", systemImage: "number")
                        MetricTile(title: "平均首字节", value: latencyAverage(total.ttfbMS), detail: "首次响应平均耗时", systemImage: "timer")
                        MetricTile(title: "平均完成耗时", value: latencyAverage(total.durationMS), detail: "请求完成平均耗时", systemImage: "clock")
                        MetricTile(title: "输出速度", value: RuntimeEventPresentation.outputSpeedDisplay(total.outputTokensPerSecond), detail: outputSpeedDetail(total.outputSpeedRequests), systemImage: "speedometer")
                        MetricTile(title: "成本", value: costText(total.cost.estimatedCostMicros, currency: total.cost.currency), detail: total.cost.complete ? "价目完整" : "部分请求未定价", systemImage: "yensign.circle")
                    }
                    HStack(spacing: 12) {
                        Picker("展示指标", selection: $trendMetric) {
                            ForEach(TrendMetric.allCases) { item in
                                Text(item.title).tag(item)
                            }
                        }
                        .frame(maxWidth: 280)
                        Spacer()
                    }
                    if !trend.points.isEmpty {
                        trendAreaChart(points: trend.points)
                        DisclosureGroup("查看时间桶明细（\(trend.points.count)）") {
                            Table(trend.points) {
                                TableColumn("时间") { point in
                                    Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: point.bucketStart)))
                                        .font(.caption.monospacedDigit())
                                }.width(min: 150, ideal: 170)
                                TableColumn("请求") { point in Text(tokenNumber(point.clientRequests)).monospacedDigit() }.width(min: 80, ideal: 90)
                                TableColumn("成功") { point in Text(tokenNumber(point.clientSuccesses)).monospacedDigit() }.width(min: 80, ideal: 90)
                                TableColumn("失败") { point in Text(tokenNumber(point.clientFailures)).monospacedDigit() }.width(min: 80, ideal: 90)
                                TableColumn("Token") { point in Text(tokenNumber(point.tokens.processedTotalTokens)).monospacedDigit() }.width(min: 100, ideal: 120)
                                TableColumn("平均首字节") { point in Text(latencyAverage(point.ttfbMS)).monospacedDigit() }.width(min: 108, ideal: 122)
                                TableColumn("平均完成耗时") { point in Text(latencyAverage(point.durationMS)).monospacedDigit() }.width(min: 108, ideal: 122)
                                TableColumn("成本") { point in Text(costText(point.cost.estimatedCostMicros, currency: point.cost.currency)).monospacedDigit() }.width(min: 110, ideal: 130)
                            }
                            .sumpterTableSurface()
                            .frame(maxWidth: .infinity)
                            .frame(height: min(260, max(90, CGFloat(trend.points.count) * 26 + 34)))
                            .padding(.top, 8)
                        }
                        .font(.callout.weight(.medium))
                    }
                }
            } else if model.runtimeV2Loading {
                HStack { ProgressView().controlSize(.small); Text("正在计算趋势…").font(.caption).foregroundStyle(.secondary) }
            } else {
                EmptyStateView(title: "暂无趋势数据", systemImage: "chart.xyaxis.line")
            }
        }
    }

    private func trendBucketValue(_ point: AdminWire.RuntimeTrendPoint, metric: TrendMetric) -> Double? {
        let completed = point.clientSuccesses + point.clientFailures + point.clientCancelled
        switch metric {
        case .clientRequests:
            return Double(point.clientRequests)
        case .successRate:
            guard completed > 0 else { return nil }
            return Double(point.clientSuccesses) * 100.0 / Double(completed)
        case .failureRate:
            guard completed > 0 else { return nil }
            return Double(point.clientFailures) * 100.0 / Double(completed)
        case .ttfbAverage:
            return point.ttfbMS.averageMS
        case .durationAverage:
            return point.durationMS.averageMS
        case .outputSpeed:
            return point.outputTokensPerSecond
        case .outputTokens:
            return Double(point.tokens.outputTokens)
        case .cacheTokenRate:
            if let rate = point.tokens.cacheReadTokenRate {
                return rate * 100.0
            }
            guard point.tokens.processedInputTokens > 0 else { return nil }
            return min(100.0, Double(max(0, point.tokens.cacheReadInputTokens)) * 100.0 / Double(point.tokens.processedInputTokens))
        case .failovers:
            return Double(point.failovers)
        case .estimatedCost:
            return Double(point.cost.estimatedCostMicros) / 1_000_000.0
        }
    }

    private func formatTrendMetricValue(_ value: Double?, metric: TrendMetric) -> String {
        guard let value, value.isFinite else { return "—" }
        switch metric {
        case .successRate, .failureRate, .cacheTokenRate:
            return String(format: "%.1f%%", value)
        case .ttfbAverage, .durationAverage:
            return RuntimeEventPresentation.durationDisplay(Int(value.rounded()))
        case .outputSpeed:
            return RuntimeEventPresentation.outputSpeedDisplay(value)
        case .estimatedCost:
            return String(format: "USD %.4f", value)
        case .clientRequests, .outputTokens, .failovers:
            return tokenNumber(Int(value.rounded()))
        }
    }

    @ViewBuilder
    private func trendAreaChart(points: [AdminWire.RuntimeTrendPoint]) -> some View {
        let keepNil = trendMetric.keepsNilForEmptyBucket
        let values: [Double?] = points.map { point in
            if let v = trendBucketValue(point, metric: trendMetric), v.isFinite {
                return max(0, v)
            }
            return keepNil ? nil : 0
        }
        let maxValue = max(1.0, values.compactMap { $0 }.max() ?? 1.0)
        let activeIdx: Int? = if let hovered = hoveredTrendBucketIndex, points.indices.contains(hovered) {
            hovered
        } else {
            values.indices.last(where: { values[$0] != nil }) ?? points.indices.last
        }

        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                HStack(spacing: 6) {
                    Circle()
                        .fill(palette.brand)
                        .frame(width: 8, height: 8)
                    Text(trendMetric.shortTitle)
                        .font(.caption.weight(.semibold))
                    Text("\(points.count) 个时间桶")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if let idx = activeIdx, points.indices.contains(idx) {
                    let bucket = points[idx]
                    HStack(spacing: 6) {
                        Text(RuntimeEventDisplay.time(Date(timeIntervalSinceReferenceDate: bucket.bucketStart)))
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(.secondary)
                        Text(formatTrendMetricValue(values[idx], metric: trendMetric))
                            .font(.caption.monospacedDigit().weight(.semibold))
                            .foregroundStyle(.primary)
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(palette.raised, in: Capsule())
                    .overlay(Capsule().stroke(palette.borderSubtle, lineWidth: 0.8))
                }
                Text("峰值 \(formatTrendMetricValue(maxValue, metric: trendMetric)) / 桶")
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            GeometryReader { geo in
                let padLeft: CGFloat = 56
                let padRight: CGFloat = 16
                let padTop: CGFloat = 12
                let padBottom: CGFloat = 24
                let chartW = max(1, geo.size.width - padLeft - padRight)
                let chartH = max(1, geo.size.height - padTop - padBottom)
                let baseY = padTop + chartH

                let coords: [(index: Int, x: CGFloat, y: CGFloat, value: Double)] = values.enumerated().compactMap { index, val in
                    guard let val else { return nil }
                    let x = padLeft + (points.count <= 1 ? chartW / 2 : (CGFloat(index) / CGFloat(points.count - 1)) * chartW)
                    let y = baseY - CGFloat(val / maxValue) * chartH
                    return (index, x, y, val)
                }

                ZStack(alignment: .topLeading) {
                    ForEach([0.0, 0.5, 1.0], id: \.self) { ratio in
                        let y = baseY - CGFloat(ratio) * chartH
                        Path { path in
                            path.move(to: CGPoint(x: padLeft, y: y))
                            path.addLine(to: CGPoint(x: padLeft + chartW, y: y))
                        }
                        .stroke(palette.borderSubtle, style: StrokeStyle(lineWidth: 0.8, dash: [4, 4]))

                        Text(formatTrendMetricValue(maxValue * ratio, metric: trendMetric))
                            .font(.system(size: 10).monospacedDigit())
                            .foregroundStyle(.secondary)
                            .position(x: padLeft / 2 - 2, y: y)
                    }

                    if coords.count >= 2 {
                        Path { path in
                            guard let first = coords.first, let last = coords.last else { return }
                            path.move(to: CGPoint(x: first.x, y: baseY))
                            for pt in coords {
                                path.addLine(to: CGPoint(x: pt.x, y: pt.y))
                            }
                            path.addLine(to: CGPoint(x: last.x, y: baseY))
                            path.closeSubpath()
                        }
                        .fill(
                            LinearGradient(
                                colors: [palette.brand.opacity(0.28), palette.brand.opacity(0.01)],
                                startPoint: .top,
                                endPoint: .bottom
                            )
                        )

                        Path { path in
                            guard let first = coords.first else { return }
                            path.move(to: CGPoint(x: first.x, y: first.y))
                            for pt in coords.dropFirst() {
                                path.addLine(to: CGPoint(x: pt.x, y: pt.y))
                            }
                        }
                        .stroke(palette.brand, style: StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
                    }

                    ForEach(coords, id: \.index) { pt in
                        let isHovered = hoveredTrendBucketIndex == pt.index
                        Circle()
                            .fill(isHovered ? palette.surface : palette.brand)
                            .frame(width: isHovered ? 9 : 6, height: isHovered ? 9 : 6)
                            .overlay(
                                Circle()
                                    .stroke(palette.brand, lineWidth: isHovered ? 2.2 : 0)
                            )
                            .position(x: pt.x, y: pt.y)
                            .onHover { hovering in
                                hoveredTrendBucketIndex = hovering ? pt.index : nil
                            }
                            .help("\(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: points[pt.index].bucketStart))) · \(formatTrendMetricValue(pt.value, metric: trendMetric))")
                    }

                    if let first = points.first {
                        Text(RuntimeEventDisplay.time(Date(timeIntervalSinceReferenceDate: first.bucketStart)))
                            .font(.system(size: 10).monospacedDigit())
                            .foregroundStyle(.secondary)
                            .position(x: padLeft + 28, y: baseY + 13)
                    }
                    if points.count > 2 {
                        let mid = points[points.count / 2]
                        Text(RuntimeEventDisplay.time(Date(timeIntervalSinceReferenceDate: mid.bucketStart)))
                            .font(.system(size: 10).monospacedDigit())
                            .foregroundStyle(.secondary)
                            .position(x: padLeft + chartW / 2, y: baseY + 13)
                    }
                    if points.count > 1, let last = points.last {
                        Text(RuntimeEventDisplay.time(Date(timeIntervalSinceReferenceDate: last.bucketStart)))
                            .font(.system(size: 10).monospacedDigit())
                            .foregroundStyle(.secondary)
                            .position(x: padLeft + chartW - 28, y: baseY + 13)
                    }
                }
            }
            .frame(height: 196)
        }
        .padding(14)
        .background(palette.inset, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }

    private func outputSpeedDetail(_ requests: Int?) -> String {
        guard let requests else { return "首字之后的生成速度" }
        return "基于 \(tokenNumber(requests)) 个可计速请求"
    }

    private func latencyAverage(_ metrics: AdminWire.RuntimeLatencyMetrics) -> String {
        guard let averageMS = metrics.averageMS else { return "—" }
        return RuntimeEventPresentation.durationDisplay(Int(averageMS.rounded()))
    }

    private func latencyDetail(
        _ metrics: AdminWire.RuntimeLatencyMetrics,
        thresholds: [Int]
    ) -> String {
        guard !thresholds.isEmpty else {
            return "已观测 \(tokenNumber(metrics.observedRequests)) 个请求"
        }
        let values = thresholds.map { threshold in
            let count = metrics.thresholdBuckets.first(where: { $0.thresholdMS == threshold })?.exceededRequests
            let label = RuntimeEventPresentation.durationDisplay(threshold)
            let value = count.map { tokenNumber($0) } ?? "—"
            return ">" + label + " " + value
        }
        return "已观测 " + tokenNumber(metrics.observedRequests) + " · " + values.joined(separator: " · ")
    }

    private func latencyThresholdPair(
        _ metrics: AdminWire.RuntimeLatencyMetrics,
        thresholds: [Int]
    ) -> String {
        thresholds.map { threshold in
            metrics.thresholdBuckets.first(where: { $0.thresholdMS == threshold })
                .map { tokenNumber($0.exceededRequests) } ?? "—"
        }.joined(separator: " / ")
    }

    @ViewBuilder
    private var v3ErrorTable: some View {
        SectionPanel(
            title: "错误聚合",
            hint: "按失败类型、阶段、入口和模型聚合；点击样本编号可下钻查看完整请求链与事件详情。"
        ) {
            if let page = model.runtimeErrorPage {
                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        Text("共 \(tokenNumber(page.totalCount)) 组").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                        Spacer()
                        SumpterPaginationControls(
                            page: page.page,
                            totalPages: page.totalPages,
                            hasPrevious: page.hasPrevious,
                            hasNext: page.hasNext,
                            pageSize: model.runtimeErrorPageSize,
                            compact: true,
                            loading: model.runtimeErrorPageLoading,
                            onPageChange: { model.loadRuntimeV2ErrorPage(page: $0) },
                            onPageSizeChange: model.setRuntimeErrorPageSize
                        )
                    }
                    if page.groups.isEmpty {
                        EmptyStateView(title: "快照内没有结构化错误", systemImage: "checkmark.circle")
                    } else {
                        Table(page.groups) {
                            TableColumn("失败") { group in
                                Text(errorFailureTitle(group)).lineLimit(2).help(group.id)
                            }.width(min: 170, ideal: 220)
                            TableColumn("入口 / 模型") { group in
                                Text(errorRouteTitle(group)).lineLimit(2)
                            }.width(min: 180, ideal: 240)
                            TableColumn("次数") { group in Text(tokenNumber(group.occurrences)).monospacedDigit() }.width(min: 72, ideal: 82)
                            TableColumn("请求") { group in Text(tokenNumber(group.affectedRequests)).monospacedDigit() }.width(min: 72, ideal: 82)
                            TableColumn("会话") { group in Text(tokenNumber(group.affectedSessions)).monospacedDigit() }.width(min: 72, ideal: 82)
                            TableColumn("样本") { group in
                                errorSampleCell(group)
                            }.width(min: 150, ideal: 210)
                            TableColumn("最近") { group in
                                Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: group.lastSeen))).font(.caption.monospacedDigit())
                            }.width(min: 150, ideal: 170)
                        }
                        .sumpterTableSurface()
                        .frame(maxWidth: .infinity)
                        .frame(height: min(240, max(90, CGFloat(page.groups.count) * 28 + 34)))
                    }
                    if let selectedID = selectedErrorSampleEventID,
                       let selectedEvent = model.runtimeEventDetail?.event ?? v2RequestChainEvents?.first(where: { $0.id == selectedID }) ?? historyEvents.first(where: { $0.id == selectedID }) {
                        let chainEvents = v2RequestChainEvents ?? [selectedEvent]
                        Divider()
                        HStack(alignment: .top, spacing: 16) {
                            RuntimeRequestTraceView(
                                chain: chainEvents,
                                truncated: model.runtimeRequestChain?.truncated ?? false,
                                selectedEventID: Binding(
                                    get: { selectedErrorSampleEventID },
                                    set: { newID in
                                        selectedErrorSampleEventID = newID
                                        model.loadRuntimeEvent(id: newID)
                                        model.loadRuntimeRequestChain(for: newID)
                                    }
                                )
                            )
                            .frame(width: 310, alignment: .topLeading)
                            Divider()
                            RuntimeEventInspector(
                                event: selectedEvent,
                                chain: chainEvents,
                                expanded: $expandedErrorEventGroups,
                                onClose: {
                                    selectedErrorSampleEventID = nil
                                    model.loadRuntimeEvent(id: nil)
                                    model.loadRuntimeRequestChain(for: nil)
                                }
                            )
                            .frame(minWidth: 360, maxWidth: .infinity, alignment: .topLeading)
                        }
                    }
                }
            } else if model.runtimeErrorPageLoading || model.runtimeV2Loading {
                HStack { ProgressView().controlSize(.small); Text("正在读取错误分组…").font(.caption).foregroundStyle(.secondary) }
            } else {
                EmptyStateView(title: "暂无错误统计数据", systemImage: "checkmark.circle")
            }
        }
    }

    private func errorFailureTitle(_ group: AdminWire.RuntimeErrorGroup) -> String {
        let kind = group.failureKind.map { dimensionValueDisplayName($0, kind: "failure_kind") }
        let phase = group.failurePhase.map { dimensionValueDisplayName($0, kind: "failure_phase") }
        return [kind, phase].compactMap { $0 }.joined(separator: " / ")
    }

    private func errorRouteTitle(_ group: AdminWire.RuntimeErrorGroup) -> String {
        [group.endpointName ?? group.endpointID, group.model].compactMap { $0 }.joined(separator: " · ")
    }

    @ViewBuilder
    private func errorSampleCell(_ group: AdminWire.RuntimeErrorGroup) -> some View {
        let samples = Array(group.sampleEventIDs.prefix(3))
        if samples.isEmpty {
            Text("—").foregroundStyle(.secondary)
        } else {
            HStack(spacing: 4) {
                ForEach(samples, id: \.self) { sampleID in
                    Button("#" + String(sampleID.prefix(8))) {
                        if selectedErrorSampleEventID == sampleID {
                            selectedErrorSampleEventID = nil
                            model.loadRuntimeEvent(id: nil)
                            model.loadRuntimeRequestChain(for: nil)
                        } else {
                            selectedErrorSampleEventID = sampleID
                            model.loadRuntimeEvent(id: sampleID)
                            model.loadRuntimeRequestChain(for: sampleID)
                        }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.mini)
                    .font(.caption2.monospaced())
                    .help("查看样本事件 \(sampleID)")
                }
            }
        }
    }

    private func applyRuntimeOverviewSort(
        _ order: [KeyPathComparator<AdminWire.RuntimeDimensionRow>],
        kind: String
    ) {
        guard let comparator = order.first else { return }
        let key = runtimeDimensionSortKey(comparator.keyPath)
        let direction = comparator.order == .reverse ? "desc" : "asc"
        switch kind {
        case "endpoint": model.setRuntimeEndpointSort(key, order: direction)
        case "project": model.setRuntimeProjectSort(key, order: direction)
        case "session": model.setRuntimeSessionSort(key, order: direction)
        default: model.setRuntimeModelSort(key, order: direction)
        }
    }

    private func runtimeDimensionSortKey(
        _ keyPath: PartialKeyPath<AdminWire.RuntimeDimensionRow>
    ) -> String {
        if keyPath == \.name { return "name" }
        if keyPath == \.requests { return "requests" }
        if keyPath == \.successRateForSorting { return "success_rate" }
        if keyPath == \.failures { return "failures" }
        if keyPath == \.inputTokens { return "input_tokens" }
        if keyPath == \.outputTokens { return "output_tokens" }
        if keyPath == \.cacheReadInputTokens { return "cache_read" }
        if keyPath == \.cacheCreationInputTokens { return "cache_write" }
        if keyPath == \.averageDurationForSorting { return "average_duration" }
        if keyPath == \.outputSpeedForSorting { return "output_tokens_per_second" }
        if keyPath == \.lastSeen { return "last_seen" }
        return model.runtimeDimensionSort
    }

    private func runtimeDimensionLabel(_ kind: String) -> String {
        switch kind {
        case "client_kind", "clientKind": "客户端"
        case "failure_kind", "failureKind": "失败类型"
        case "failure_phase", "failurePhase": "失败阶段"
        case "stream_terminal", "streamTerminal": "流终止"
        case "endpoint": "入口"
        case "model": "模型"
        case "purpose": "用途"
        case "protocol": "协议"
        case "project": "项目"
        case "session": "会话"
        default: kind
        }
    }

    private func runtimeDimensionDisplayName(_ name: String, kind: String) -> String {
        dimensionValueDisplayName(name, kind: kind)
    }

    private func dimensionTokenText(_ value: Int, related: Int) -> String {
        // Dimension projection currently exposes aggregate values without
        // per-field presence counters. Preserve explicit non-zero values and
        // use an em dash for an entirely unobserved field rather than inventing
        // a zero for old records.
        guard value != 0 || related == 0 else { return "—" }
        return tokenNumber(value)
    }

    private func dimensionCacheTokenRate(_ row: AdminWire.RuntimeDimensionRow) -> Double? {
        if let value = row.cacheReadTokenRate { return value }
        guard let processed = row.processedInputTokens, processed > 0 else { return nil }
        return min(1, Double(max(0, row.cacheReadInputTokens)) / Double(processed))
    }

    @ViewBuilder
    private var exportControls: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) { exportControlItems }
            VStack(alignment: .leading, spacing: 8) { exportControlItems }
        }
    }

    @ViewBuilder
    private var exportControlItems: some View {
        // Use a content-sized Menu instead of the native Picker button.  A
        // Picker in a compact HStack can still collapse its value to "…"
        // after SwiftUI applies the platform control's minimum width; the
        // explicit label below owns its intrinsic width and never truncates.
        HStack(spacing: 4) {
            Text("范围").font(.callout.weight(.semibold))
            Menu {
                Button { exportScope = "events" } label: {
                    exportMenuItem("事件", selected: exportScope == "events")
                }
                Button { exportScope = "projects" } label: {
                    exportMenuItem("项目", selected: exportScope == "projects")
                }
                Button { exportScope = "sessions" } label: {
                    exportMenuItem("会话", selected: exportScope == "sessions")
                }
            } label: {
                exportMenuLabel(exportScopeTitle)
            }
        }
        HStack(spacing: 4) {
            Text("格式").font(.callout.weight(.semibold))
            Menu {
                Button { exportFormat = "jsonl" } label: {
                    exportMenuItem("JSONL", selected: exportFormat == "jsonl")
                }
                Button { exportFormat = "csv" } label: {
                    exportMenuItem("CSV", selected: exportFormat == "csv")
                }
            } label: {
                exportMenuLabel(exportFormat.uppercased())
            }
        }
        HStack(spacing: 4) {
            Text("数据范围").font(.callout.weight(.semibold))
            Menu {
                Button { exportPrivacy = "stored" } label: {
                    exportMenuItem("源数据（默认）", selected: exportPrivacy == "stored")
                }
                Button { exportPrivacy = "redacted" } label: {
                    exportMenuItem("脱敏副本", selected: exportPrivacy == "redacted")
                }
            } label: {
                exportMenuLabel(exportPrivacyTitle)
            }
        }
        Button("估算") {
            if exportPrivacy == "stored" {
                confirmStoredEstimate = true
            } else {
                model.estimateRuntimeExport(scope: exportScope, format: exportFormat, privacy: "redacted", confirmStored: false)
            }
        }
        .disabled(model.runtimeHistoryPage == nil)
        Button {
            if exportPrivacy == "stored" {
                confirmStoredExport = true
            } else {
                model.exportRuntimeAnalytics(scope: exportScope, format: exportFormat, privacy: exportPrivacy)
            }
        } label: {
            Label(model.runtimeExportBusy ? "导出中…" : "导出", systemImage: "arrow.down.doc")
        }
        .disabled(model.runtimeExportBusy || !exportEstimateMatchesSelection)
    }

    private var exportScopeTitle: String {
        switch exportScope {
        case "projects": "项目"
        case "sessions": "会话"
        default: "事件"
        }
    }

    private var exportPrivacyTitle: String {
        exportPrivacy == "redacted" ? "脱敏副本" : "源数据（默认）"
    }

    private func exportMenuLabel(_ title: String) -> some View {
        HStack(spacing: 5) {
            Text(title)
                .lineLimit(1)
            Image(systemName: "chevron.up.chevron.down")
                .font(.caption2.weight(.semibold))
        }
        .fixedSize(horizontal: true, vertical: false)
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .foregroundStyle(.tint)
        .background(Color.accentColor.opacity(0.10), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .stroke(Color.accentColor.opacity(0.18), lineWidth: 0.8)
        }
        .accessibilityLabel(title)
    }

    @ViewBuilder
    private func exportMenuItem(_ title: String, selected: Bool) -> some View {
        if selected {
            Label(title, systemImage: "checkmark")
        } else {
            Text(title)
        }
    }

    private var exportEstimateMatchesSelection: Bool {
        guard let estimate = model.runtimeExportEstimate else { return false }
        return estimate.scope == exportScope && estimate.format == exportFormat && estimate.privacy == exportPrivacy
    }

    private func tokenNumber(_ value: Int) -> String {
        RuntimeEventPresentation.tokenCountDisplay(value) // stable en_US grouping, including 0
    }

    private func costText(_ micros: Int, currency: String?) -> String {
        let amount = Double(micros) / 1_000_000.0
        let value = String(format: "%.4f", amount)
        return "\(currency ?? "") \(value)".trimmingCharacters(in: .whitespaces)
    }

    @ViewBuilder
    private var analyticsFilterStatus: some View {
        let values = activeRuntimeFilterLabels
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) {
                filterStatusLabel(values: values)
                Spacer(minLength: 0)
            }
            VStack(alignment: .leading, spacing: 4) {
                filterStatusLabel(values: values)
            }
        }
        .font(.caption)
        .fixedSize(horizontal: false, vertical: true)
        .frame(minHeight: 22, alignment: .leading)
        .transaction { transaction in transaction.animation = nil }
    }

    /// The top-level reset must stay available for both global picker filters
    /// and the local project/session drill-down created by clicking a table
    /// row. The latter does not change the aggregate snapshot filter, but it
    /// still changes the visible session/model tables and must be clearable
    /// from the same control surface.
    private var hasActiveRuntimeFilters: Bool {
        let globalFilters = [
            model.runtimeAnalyticsRequestKind,
            model.runtimeAnalyticsClientKind,
            model.runtimeAnalyticsClientVariant, model.runtimeAnalyticsAgentRole, model.runtimeAnalyticsAgentName, model.runtimeAnalyticsParentThreadID, model.runtimeAnalyticsParentTurnID, model.runtimeAnalyticsRootTurnID,
            model.runtimeAnalyticsEndpointID,
            model.runtimeAnalyticsProject,
            model.runtimeAnalyticsSessionID,
            model.runtimeAnalyticsModel,
            model.runtimeAnalyticsRequestPurpose,
            model.runtimeAnalyticsOutcome,
            model.runtimeAnalyticsFailureKind,
            model.runtimeAnalyticsFailurePhase,
        ]
        return globalFilters.contains { !$0.isEmpty }
            || !model.runtimeLocalProjectID.isEmpty
            || !model.runtimeLocalProjectName.isEmpty
            || !model.runtimeLocalSessionID.isEmpty
            || !model.runtimeLocalSessionName.isEmpty
    }

    private var advancedRuntimeFilterLabels: [String] {
        [
            model.runtimeAnalyticsClientVariant.isEmpty ? nil : "客户端变体=\(model.runtimeAnalyticsClientVariant)",
            model.runtimeAnalyticsAgentRole.isEmpty ? nil : "代理角色=\(model.runtimeAnalyticsAgentRole)",
            model.runtimeAnalyticsAgentName.isEmpty ? nil : "代理名称=\(model.runtimeAnalyticsAgentName)",
            model.runtimeAnalyticsParentThreadID.isEmpty ? nil : "父线程=\(model.runtimeAnalyticsParentThreadID)",
            model.runtimeAnalyticsParentTurnID.isEmpty ? nil : "父回合=\(model.runtimeAnalyticsParentTurnID)",
            model.runtimeAnalyticsRootTurnID.isEmpty ? nil : "根回合=\(model.runtimeAnalyticsRootTurnID)",
            model.runtimeAnalyticsRequestPurpose.isEmpty ? nil : "用途=\(model.runtimeAnalyticsRequestPurpose)",
            model.runtimeAnalyticsFailureKind.isEmpty ? nil : "失败类型=\(model.runtimeAnalyticsFailureKind)",
            model.runtimeAnalyticsFailurePhase.isEmpty ? nil : "失败阶段=\(model.runtimeAnalyticsFailurePhase)",
        ].compactMap { $0 }
    }

    private var activeRuntimeFilterLabels: [String] {
        [
            model.runtimeAnalyticsClientVariant.isEmpty ? nil : "客户端变体=\(model.runtimeAnalyticsClientVariant)",
            model.runtimeAnalyticsAgentRole.isEmpty ? nil : "代理角色=\(model.runtimeAnalyticsAgentRole)",
            model.runtimeAnalyticsAgentName.isEmpty ? nil : "代理名称=\(model.runtimeAnalyticsAgentName)",
            model.runtimeAnalyticsParentThreadID.isEmpty ? nil : "父线程=\(model.runtimeAnalyticsParentThreadID)",
            model.runtimeAnalyticsParentTurnID.isEmpty ? nil : "父回合=\(model.runtimeAnalyticsParentTurnID)",
            model.runtimeAnalyticsRootTurnID.isEmpty ? nil : "根回合=\(model.runtimeAnalyticsRootTurnID)",
            model.runtimeAnalyticsRequestKind.isEmpty ? nil : "请求类型=\(RuntimeEventPresentation.observedRequestKind(model.runtimeAnalyticsRequestKind))",
            model.runtimeAnalyticsClientKind.isEmpty ? nil : "客户端=\(clientDisplayName(model.runtimeAnalyticsClientKind))",
            model.runtimeAnalyticsEndpointID.isEmpty ? nil : "入口=\(model.runtimeAnalyticsEndpointID)",
            model.runtimeAnalyticsProject.isEmpty ? nil : "项目=\(projectDisplayName(model.runtimeAnalyticsProject))",
            model.runtimeAnalyticsSessionID.isEmpty ? nil : "会话=\(sessionDisplayName(model.runtimeAnalyticsSessionID))",
            model.runtimeAnalyticsModel.isEmpty ? nil : "模型=\(model.runtimeAnalyticsModel)",
            model.runtimeAnalyticsRequestPurpose.isEmpty ? nil : "用途=\(dimensionValueDisplayName(model.runtimeAnalyticsRequestPurpose, kind: "purpose"))",
            model.runtimeAnalyticsOutcome.isEmpty ? nil : "最终结果=\(runtimeOutcomeDisplayName(model.runtimeAnalyticsOutcome))",
            model.runtimeAnalyticsFailureKind.isEmpty ? nil : "失败类型=\(dimensionValueDisplayName(model.runtimeAnalyticsFailureKind, kind: "failure_kind"))",
            model.runtimeAnalyticsFailurePhase.isEmpty ? nil : "失败阶段=\(dimensionValueDisplayName(model.runtimeAnalyticsFailurePhase, kind: "failure_phase"))",
            model.runtimeLocalProjectName.isEmpty ? nil : "项目下钻=\(projectDisplayName(model.runtimeLocalProjectName))",
            model.runtimeLocalSessionName.isEmpty ? nil : "会话下钻=\(sessionDisplayName(model.runtimeLocalSessionName))",
        ].compactMap { $0 }
    }

    @ViewBuilder
    private func filterStatusLabel(values: [String]) -> some View {
        Label(values.isEmpty ? "筛选未应用" : "筛选已应用", systemImage: values.isEmpty ? "line.3.horizontal.decrease.circle" : "line.3.horizontal.decrease.circle.fill")
            .foregroundStyle(values.isEmpty ? Color.secondary : Color.green)
        Text(values.isEmpty ? "全部数据" : values.joined(separator: " · "))
            .foregroundStyle(.secondary)
            .lineLimit(dynamicTypeSize.isAccessibilitySize ? 4 : 2)
            .fixedSize(horizontal: false, vertical: true)
        if model.runtimeV2Loading || model.runtimeDimensionPageLoading {
            Text("更新中")
                .foregroundStyle(.secondary)
        }
        if let error = model.runtimeV2Error, !error.isEmpty {
            Text("读取失败：\(error)")
                .foregroundStyle(.orange)
                .textSelection(.enabled)
        }
    }

    private func analyticsRows(
        _ rows: [AdminWire.RuntimeAnalytics.DimensionRow]?,
        fallback: [UsageAggregateRow]
    ) -> [UsageAggregateRow] {
        guard let rows else { return fallback }
        return rows.map { row in
            UsageAggregateRow(
                id: row.name,
                name: row.name,
                attempts: row.attempts,
                successes: row.successes,
                failures: row.failures,
                cancelled: row.cancelled,
                failovers: row.failovers,
                averageMS: Int((row.averageDurationMS ?? 0).rounded()),
                inputTokens: row.inputTokens ?? 0,
                outputTokens: row.outputTokens ?? 0,
                cacheReadInputTokens: row.cacheReadInputTokens ?? 0,
                inputTokenPresence: row.usageFieldPresence?.inputTokens,
                outputTokenPresence: row.usageFieldPresence?.outputTokens,
                cacheReadTokenPresence: row.usageFieldPresence?.cacheReadInputTokens,
                processedInputTokens: row.processedInputTokens ?? 0,
                processedTotalTokens: row.processedTotalTokens ?? 0,
                cacheCreationInputTokens: row.cacheCreationInputTokens ?? 0,
                cacheReadTokenRate: row.cacheReadTokenRate,
                cacheReadRequestRate: row.cacheReadRequestRate,
                cacheCreationTokenRate: row.cacheCreationTokenRate,
                cacheCreationTokenPresence: row.usageFieldPresence?.cacheCreationInputTokens,
                tokenAccountingSemantics: row.tokenAccountingSemantics
            )
        }
        .sorted(using: UsageAggregateRow.stableOrder)
    }

    private func analyticsCountRows(
        _ rows: [AdminWire.RuntimeAnalytics.CountRow]?,
        fallback: [UsageAggregateRow]
    ) -> [UsageAggregateRow] {
        guard let rows else { return fallback }
        return rows.map { row in
            UsageAggregateRow(
                id: row.name,
                name: row.name,
                attempts: row.count,
                successes: 0,
                failures: 0,
                failovers: 0,
                averageMS: 0
            )
        }
        .sorted(using: UsageAggregateRow.stableOrder)
    }

    private var healthPanel: some View {
        HStack(spacing: 12) {
            Text("即时健康")
                .font(.subheadline.weight(.semibold))
            StatusBadge(
                text: Self.healthLabel(displayHealth.state),
                systemImage: Self.healthIcon(displayHealth.state),
                color: Self.healthColor(displayHealth.state)
            )
            Text(displayHealth.headline)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(dynamicTypeSize.isAccessibilitySize ? 4 : 2)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
    }

    private static func healthLabel(_ state: ProxyHealth) -> String {
        switch state {
        case .healthy: "正常"
        case .idle: "空闲"
        case .degraded: "降级"
        case .down: "故障"
        case .stopped: "已停止"
        }
    }

    private static func healthIcon(_ state: ProxyHealth) -> String {
        switch state {
        case .healthy: "checkmark.circle.fill"
        case .idle: "moon.zzz"
        case .degraded: "exclamationmark.triangle.fill"
        case .down: "xmark.octagon.fill"
        case .stopped: "pause.circle.fill"
        }
    }

    private static func healthColor(_ state: ProxyHealth) -> Color {
        switch state {
        case .healthy: .green
        case .idle: .secondary
        case .degraded: .orange
        case .down: .red
        case .stopped: .secondary
        }
    }

    private var diagnosticPanel: some View {
        SectionPanel(
            title: "结构化诊断",
            hint: "次要排障维度来自事件事实字段，不从模型名、HTTP 200 或自由文本推断。切换标签一次只看一个维度。"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 170), spacing: 12)], spacing: 12) {
                    MetricTile(
                        title: "已识别失败原因",
                        value: "\(analytics?.failureKinds?.reduce(0) { $0 + $1.attempts } ?? diagnosticSummary.structuredFailureEvents) / \(analytics?.clientFailures ?? diagnosticSummary.failedEvents)",
                        detail: "结构化原因 / 失败事件",
                        systemImage: "exclamationmark.triangle"
                    )
                    MetricTile(
                        title: "实际工具调用",
                        value: "\(analytics?.toolCalls?.reduce(0) { $0 + $1.count } ?? diagnosticSummary.toolCallCount)",
                        detail: "来自 \(diagnosticSummary.toolCallEvents) 个客户端事件",
                        systemImage: "wrench.and.screwdriver"
                    )
                    MetricTile(
                        title: "流协议追踪",
                        value: "\(analytics?.streamTerminals?.reduce(0) { $0 + $1.attempts } ?? diagnosticSummary.streamedEvents)",
                        detail: streamDiagnosticDetail,
                        systemImage: "waveform.path.ecg"
                    )
                    MetricTile(
                        title: "Codex 元数据",
                        value: "\(analytics?.codexMetadataPresent ?? diagnosticSummary.codexEvents)",
                        detail: "请求类型、代理、工作区与工具命名空间",
                        systemImage: "shippingbox"
                    )
                    MetricTile(
                        title: "后台功能请求",
                        value: "\(analytics?.internalFeatureRequests ?? 0)",
                        detail: "不混入普通项目统计",
                        systemImage: "gearshape.2"
                    )
                }

                if let requests = analytics?.codexRequests {
                    Text("Codex 逻辑请求（独立于连接计数）").font(.headline)
                    Text(requests.counts.keys.sorted().map { "\(RuntimeEventPresentation.codexRequestKind($0)) \(requests.counts[$0] ?? 0)" }.joined(separator: " · "))
                    Text("WS 在连接结束后汇总；未观测连接 \(requests.unobservedWebsocketConnections) · 未观测帧 \(requests.unobservedFrames)").font(.caption).foregroundStyle(.secondary)
                }

                HStack(spacing: 12) {
                    Picker("诊断维度", selection: $diagnosticDimension) {
                        ForEach(DiagnosticDimension.allCases) { dimension in
                            Text(dimension.title).tag(dimension)
                        }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    Spacer(minLength: 0)
                    Text(clientRecordingSummary)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                diagnosticTable
            }
        }
    }

    private var streamDiagnosticDetail: String {
        let gap = diagnosticSummary.maxChunkGapMS.map { RuntimeEventPresentation.durationDisplay($0) } ?? "未记录"
        let terminal = diagnosticSummary.missingTerminalEvents == 0
            ? "终止事件齐全"
            : "\(diagnosticSummary.missingTerminalEvents) 条未观察到终止"
        return "\(diagnosticSummary.observedChunks) chunks · 最大间隔 \(gap) · \(terminal)"
    }

    private var clientRecordingSummary: String {
        "未知客户端 \(diagnosticSummary.explicitUnknownClients) · 未记录 \(diagnosticSummary.unrecordedClientKinds)"
    }

    @ViewBuilder
    private var diagnosticTable: some View {
        switch diagnosticDimension {
        case .failures:
            diagnosticAggregateTable(
                rows: analyticsRows(
                    analytics?.failureKinds,
                    fallback: UsageAggregateRow.failureRows(from: displayRuntime.recentEvents)
                ),
                sort: $failureSort,
                emptyTitle: "最近窗口没有失败事件"
            )
        case .routing:
            routingDiagnosticTables
        case .tools:
            diagnosticAggregateTable(
                rows: analyticsCountRows(
                    analytics?.toolCalls,
                    fallback: UsageAggregateRow.toolRows(from: displayRuntime.recentEvents)
                ),
                sort: $toolSort,
                emptyTitle: "最近窗口未观察到实际工具调用"
            )
        case .stream:
            diagnosticAggregateTable(
                rows: analyticsRows(
                    analytics?.streamTerminals,
                    fallback: UsageAggregateRow.streamTerminalRows(from: displayRuntime.recentEvents)
                ),
                sort: $streamSort,
                emptyTitle: "最近窗口没有流协议追踪"
            )
        case .codex:
            diagnosticAggregateTable(
                rows: UsageAggregateRow.codexRows(from: displayRuntime.recentEvents),
                sort: $codexSort,
                emptyTitle: "最近窗口没有 Codex 元数据"
            )
        }
    }

    private var routingDiagnosticTables: some View {
        VStack(alignment: .leading, spacing: 12) {
            diagnosticSubsection(
                title: "协议路径（Source → Target · 模式）",
                rows: analyticsRows(
                    analytics?.protocolRoutes,
                    fallback: UsageAggregateRow.protocolRouteRows(from: displayRuntime.recentEvents)
                ),
                sort: $protocolRouteSort,
                emptyTitle: "最近窗口没有协议路径记录"
            )
            diagnosticSubsection(
                title: "命中特征规则",
                rows: analyticsRows(
                    analytics?.featureRules,
                    fallback: UsageAggregateRow.featureRuleRows(from: displayRuntime.recentEvents)
                ),
                sort: $featureRuleSort,
                emptyTitle: "最近窗口没有客户端路由记录"
            )
            diagnosticSubsection(
                title: "上游 HTTP 状态",
                rows: analyticsRows(
                    analytics?.upstreamStatuses,
                    fallback: UsageAggregateRow.upstreamStatusRows(from: displayRuntime.recentEvents)
                ),
                sort: $upstreamStatusSort,
                emptyTitle: "最近窗口没有上游尝试"
            )
        }
    }

    private func diagnosticSubsection(
        title: String,
        rows: [UsageAggregateRow],
        sort: Binding<[KeyPathComparator<UsageAggregateRow>]>,
        emptyTitle: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            diagnosticAggregateTable(rows: rows, sort: sort, emptyTitle: emptyTitle)
        }
    }

    @ViewBuilder
    private func diagnosticAggregateTable(
        rows: [UsageAggregateRow],
        sort: Binding<[KeyPathComparator<UsageAggregateRow>]>,
        emptyTitle: String
    ) -> some View {
        if rows.isEmpty {
            EmptyStateView(title: emptyTitle, systemImage: "waveform.path.ecg")
        } else {
            Table(rows.sorted(using: sort.wrappedValue), sortOrder: sort) {
                TableColumn("维度", value: \.name) { row in
                    Text(row.name).lineLimit(2).help(row.name)
                }
                TableColumn("事件", value: \.attempts) { row in
                    Text("\(row.attempts)").monospacedDigit()
                }
                TableColumn("成功", value: \.successes) { row in
                    Text("\(row.successes)").monospacedDigit()
                }
                TableColumn("失败", value: \.failures) { row in
                    Text("\(row.failures)").monospacedDigit()
                }
                TableColumn("平均耗时", value: \.averageMS) { row in
                    Text(RuntimeEventPresentation.durationDisplay(row.averageMS)).monospacedDigit()
                }
                TableColumn("FO", value: \.failovers) { row in
                    Text("\(row.failovers)").monospacedDigit()
                }
            }
            .sumpterTableSurface()
            .frame(height: adaptiveTableHeight(rows: rows.count, max: 240))
        }
    }
}
