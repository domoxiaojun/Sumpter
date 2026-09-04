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

    @ObservedObject var model: AppModel
    /// Keep the overview KPI groups on one visual baseline.  The value is a
    /// minimum rather than a fixed frame so larger Dynamic Type sizes can
    /// still expand cards without clipping their details.
    private let overviewMetricMinimumHeight: CGFloat = 112
    @State private var cleanupPresented = false
    // 关闭自动刷新时冻结的画面快照;nil 表示跟随实时数据(后台轮询仍在更新模型)。
    @State private var frozenRuntime: RuntimeSnapshot?
    @State private var frozenHealth: ProxyHealthSummary?
    // 排行表的排序由用户掌握,默认是恒定的名称序 —— 见 UsageAggregateRow.stableOrder。
    @State private var endpointSort = UsageAggregateRow.stableOrder
    @State private var modelSort = UsageAggregateRow.stableOrder
    @State private var clientKindSort = UsageAggregateRow.stableOrder
    @State private var purposeSort = UsageAggregateRow.stableOrder
    @State private var failureSort = UsageAggregateRow.stableOrder
    @State private var protocolRouteSort = UsageAggregateRow.stableOrder
    @State private var featureRuleSort = UsageAggregateRow.stableOrder
    @State private var upstreamStatusSort = UsageAggregateRow.stableOrder
    @State private var toolSort = UsageAggregateRow.stableOrder
    @State private var streamSort = UsageAggregateRow.stableOrder
    @State private var codexSort = UsageAggregateRow.stableOrder
    @State private var endpointDetailSort: [KeyPathComparator<AdminWire.RuntimeAnalytics.DimensionRow>] = [KeyPathComparator(\.name)]
    @State private var projectSort: [KeyPathComparator<AdminWire.RuntimeAnalytics.DimensionRow>] = [KeyPathComparator(\.name)]
    @State private var sessionSort: [KeyPathComparator<AdminWire.RuntimeAnalytics.DimensionRow>] = [KeyPathComparator(\.name)]
    @State private var runtimeDimensionTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var runtimeEndpointTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var runtimeProjectTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var runtimeSessionTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var runtimeModelTableSort: [KeyPathComparator<AdminWire.RuntimeDimensionRow>] = [KeyPathComparator(\.lastSeen, order: .reverse)]
    @State private var diagnosticDimension: DiagnosticDimension = .failures
    @State private var confirmDeleteSessionID: String?
    @State private var confirmUnidentifiedSessionID: String?
    @State private var unidentifiedConfirmationPhrase = ""
    @State private var locatingProjectName: String?
    @State private var confirmStoredExport = false
    @State private var exportPrivacy = "stored"
    @State private var exportScope = "events"
    @State private var exportFormat = "jsonl"
    @State private var confirmStoredEstimate = false
    @State private var selectedBoard: StatisticsBoard = .overview
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
    private var operationalSummary: RuntimeOperationalSummary {
        RuntimeOperationalSummary(events: displayRuntime.recentEvents)
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
        let existing = Dictionary(uniqueKeysWithValues: displayRuntime.recentEvents.map { ($0.id, $0) })
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
        let existing = Dictionary(uniqueKeysWithValues: historyEvents.map { ($0.id, $0) })
        return chain.events.map { $0.mergedRuntimeEvent(with: existing[$0.id]) }
    }

    /// 开关或间隔一变就重启刷新循环。
    private var autoRefreshTaskID: String {
        "\(model.autoRefreshEnabled)-\(model.statisticsAutoRefreshIntervalSeconds)"
    }

    var body: some View {
        SettingsPage(title: "统计", subtitle: "请求统计") {
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
        .onChange(of: model.runtimeDimensionSort) { _, sort in
            runtimeDimensionTableSort = [runtimeDimensionComparator(sort: sort, order: model.runtimeDimensionOrder)]
        }
        .onChange(of: model.runtimeDimensionOrder) { _, order in
            runtimeDimensionTableSort = [runtimeDimensionComparator(sort: model.runtimeDimensionSort, order: order)]
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
                Text("筛选条件")
                    .font(.subheadline.weight(.semibold))
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 180), spacing: 12)], spacing: 10) {
                    analyticsFilterPickers
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
            Text("不限入口").tag("")
            ForEach(facets?.endpoints ?? []) { item in
                Text("\(model.config.endpoints.first(where: { $0.id == item.value })?.name ?? item.value) · \(item.count)")
                    .tag(item.value)
            }
        }
        Picker("项目", selection: Binding(
            get: { model.runtimeAnalyticsProject },
            set: { model.setRuntimeAnalyticsFilters(project: $0, sessionID: "") }
        )) {
            Text("不限项目").tag("")
            ForEach(facets?.projects ?? []) { item in
                Text("\(projectDisplayName(item.value)) · \(item.count)").tag(item.value)
            }
        }
        Picker("会话", selection: Binding(
            get: { model.runtimeAnalyticsSessionID },
            set: { model.setRuntimeAnalyticsFilters(sessionID: $0) }
        )) {
            Text("不限会话").tag("")
            ForEach(facets?.sessions ?? []) { item in
                Text("\(sessionDisplayName(item.value)) · \(item.count)").tag(item.value)
            }
        }
        Picker("客户端", selection: Binding(
            get: { model.runtimeAnalyticsClientKind },
            set: { model.setRuntimeAnalyticsFilters(clientKind: $0) }
        )) {
            Text("不限客户端").tag("")
            ForEach(facets?.clientKinds ?? []) { item in
                Text("\(clientDisplayName(item.value)) · \(item.count)").tag(item.value)
            }
        }
        Picker("模型", selection: Binding(
            get: { model.runtimeAnalyticsModel },
            set: { model.setRuntimeAnalyticsFilters(model: $0) }
        )) {
            Text("不限模型").tag("")
            ForEach(facets?.models ?? []) { item in
                Text("\(item.value) · \(item.count)").tag(item.value)
            }
        }
        Picker("最终结果", selection: Binding(
            get: { model.runtimeAnalyticsOutcome },
            set: { model.setRuntimeAnalyticsFilters(outcome: $0) }
        )) {
            Text("不限结果").tag("")
            Text("成功").tag("succeeded")
            Text("失败").tag("failed")
            Text("已取消").tag("cancelled")
        }
        Picker("用途", selection: Binding(
            get: { model.runtimeAnalyticsRequestPurpose },
            set: { model.setRuntimeAnalyticsFilters(requestPurpose: $0) }
        )) {
            Text("不限用途").tag("")
            ForEach(facets?.requestPurposes ?? []) { item in
                Text("\(dimensionValueDisplayName(item.value, kind: "purpose")) · \(item.count)").tag(item.value)
            }
        }
        Picker("失败类型", selection: Binding(
            get: { model.runtimeAnalyticsFailureKind },
            set: { model.setRuntimeAnalyticsFilters(failureKind: $0) }
        )) {
            Text("不限失败类型").tag("")
            ForEach(facets?.failureKinds ?? []) { item in
                Text("\(dimensionValueDisplayName(item.value, kind: "failure_kind")) · \(item.count)").tag(item.value)
            }
        }
        Picker("失败阶段", selection: Binding(
            get: { model.runtimeAnalyticsFailurePhase },
            set: { model.setRuntimeAnalyticsFilters(failurePhase: $0) }
        )) {
            Text("不限失败阶段").tag("")
            ForEach(facets?.failurePhases ?? []) { item in
                Text("\(dimensionValueDisplayName(item.value, kind: "failure_phase")) · \(item.count)").tag(item.value)
            }
        }
        Button("清除筛选") {
            model.setRuntimeAnalyticsFilters(clientKind: "", endpointID: "", project: "", sessionID: "", model: "", requestPurpose: "", outcome: "", failureKind: "", failurePhase: "")
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
            hint: "按顺序查看数据存储、核心指标、Token 与缓存，以及入口、项目、会话和模型使用情况。项目或会话行可钻取模型明细。"
        ) {
            VStack(alignment: .leading, spacing: 14) {
            storageOverviewPanel
                Divider()
                healthPanel
                Divider()
                overviewRequestSummary
                Divider()
                overviewTokenSummary
                Divider()
                overviewDimensionTables
            }
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

    private func runtimeOverviewUsageTable(_ page: AdminWire.RuntimeDimensionPage, kind: String, sortOrder: Binding<[KeyPathComparator<AdminWire.RuntimeDimensionRow>]>) -> some View {
        Table(page.rows, selection: runtimeOverviewTableSelection(page: page, kind: kind), sortOrder: sortOrder) {
            TableColumn(runtimeDimensionLabel(kind), value: \.name) { row in runtimeOverviewDimensionName(row, kind: kind) }.width(min: 190, ideal: 260)
            TableColumn("请求", value: \.requests) { row in Text(tokenNumber(row.requests)).monospacedDigit() }.width(min: 76, ideal: 88)
            TableColumn("成功率", value: \.successRateForSorting) { row in Text(rateText(row.successRateForSorting >= 0 ? row.successRateForSorting : nil)).monospacedDigit() }.width(min: 92, ideal: 106)
            TableColumn("失败", value: \.failures) { row in Text(tokenNumber(row.failures)).monospacedDigit() }.width(min: 76, ideal: 88)
            TableColumn("输入 Token", value: \.inputTokens) { row in Text(dimensionTokenText(row.inputTokens, related: row.requests)).monospacedDigit() }.width(min: 108, ideal: 126)
            TableColumn("输出 Token", value: \.outputTokens) { row in Text(dimensionTokenText(row.outputTokens, related: row.requests)).monospacedDigit() }.width(min: 108, ideal: 126)
            TableColumn("缓存读取", value: \.cacheReadInputTokens) { row in
                VStack(alignment: .leading, spacing: 2) {
                    Text(dimensionTokenText(row.cacheReadInputTokens, related: row.requests)).monospacedDigit()
                    Text("命中率 " + rateText(dimensionCacheTokenRate(row))).font(.caption2.monospacedDigit()).foregroundStyle(.secondary)
                }
            }.width(min: 114, ideal: 136)
            TableColumn("缓存写入", value: \.cacheCreationInputTokens) { row in Text(dimensionTokenText(row.cacheCreationInputTokens, related: row.requests)).monospacedDigit() }.width(min: 108, ideal: 126)
            TableColumn("平均耗时", value: \.averageDurationForSorting) { row in Text(row.averageDurationMS.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) } ?? "—").monospacedDigit() }.width(min: 108, ideal: 126)
            TableColumn("最近活动", value: \.lastSeen) { row in Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: row.lastSeen))).font(.caption.monospacedDigit()) }.width(min: 150, ideal: 174)
        }
        .onChange(of: sortOrder.wrappedValue) { _, order in applyRuntimeOverviewSort(order, kind: kind) }
        .sumpterTableSurface()
        .frame(minWidth: 1_040)
        .frame(height: min(280, max(96, CGFloat(page.rows.count) * 31 + 38)))
    }

    private func runtimeOverviewCostTable(_ page: AdminWire.RuntimeDimensionPage, kind: String, sortOrder: Binding<[KeyPathComparator<AdminWire.RuntimeDimensionRow>]>) -> some View {
        Table(page.rows, selection: runtimeOverviewTableSelection(page: page, kind: kind), sortOrder: sortOrder) {
            TableColumn(runtimeDimensionLabel(kind), value: \.name) { row in runtimeOverviewDimensionName(row, kind: kind) }.width(min: 210, ideal: 280)
            // Cost is calculated after the server applies its supported
            // aggregate ordering; keep it read-only here instead of sorting
            // only the visible page on the client.
            TableColumn("成本") { row in Text(costText(row.cost?.estimatedCostMicros ?? 0, currency: row.cost?.currency)).monospacedDigit() }.width(min: 110, ideal: 130)
            TableColumn("请求", value: \.requests) { row in Text(tokenNumber(row.requests)).monospacedDigit() }.width(min: 76, ideal: 88)
            TableColumn("暂无法计价") { row in Text(tokenNumber((row.cost?.unpricedRequests ?? 0) + (row.cost?.unknownAccountingRequests ?? 0))).monospacedDigit() }.width(min: 110, ideal: 126)
            TableColumn("最近活动", value: \.lastSeen) { row in Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: row.lastSeen))).font(.caption.monospacedDigit()) }.width(min: 150, ideal: 174)
        }
        .onChange(of: sortOrder.wrappedValue) { _, order in applyRuntimeOverviewSort(order, kind: kind) }
        .sumpterTableSurface()
        .frame(minWidth: 720)
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
        let content = VStack(alignment: .leading, spacing: 2) {
            Text(runtimeDimensionDisplayName(row.name, kind: kind)).lineLimit(1).help(row.key)
            Text(dimensionSourceDisplayName(row.source, kind: kind)).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
        }
        if kind == "project" {
            Button { model.setRuntimeLocalProject(row.key, projectName: row.name) } label: { content }
                .buttonStyle(.link)
                .help("仅筛选下方会话：\(row.key)")
                .accessibilityLabel("仅查看项目 \(runtimeDimensionDisplayName(row.name, kind: kind)) 的会话和模型用量")
        } else if kind == "session" {
            Button { model.setRuntimeLocalSession(row.key, sessionName: row.name) } label: { content }
                .buttonStyle(.link)
                .help("仅查看会话：\(row.key) 的模型用量")
                .accessibilityLabel("仅查看会话 \(runtimeDimensionDisplayName(row.name, kind: kind)) 的模型用量")
        } else { content }
    }

    private var costBoard: some View {
        SectionPanel(title: "成本", hint: "先看成本摘要，再比较入口、项目、会话和模型成本；项目或会话行可钻取模型明细。") {
            VStack(alignment: .leading, spacing: 14) {
                costSummary
                Divider()
                runtimeOverviewDimensionTable(kind: "endpoint", title: "入口成本", page: model.runtimeEndpointsPage, sortOrder: $runtimeEndpointTableSort, mode: .cost)
                runtimeOverviewDimensionTable(kind: "project", title: "项目成本", page: model.runtimeProjectsPage, sortOrder: $runtimeProjectTableSort, mode: .cost)
                runtimeOverviewDimensionTable(kind: "session", title: "会话成本", page: model.runtimeSessionsPage, sortOrder: $runtimeSessionTableSort, mode: .cost)
                runtimeOverviewDimensionTable(kind: "model", title: "模型成本", page: model.runtimeModelsPage, sortOrder: $runtimeModelTableSort, mode: .cost)
            }
        }
    }

    private var costSummary: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("成本摘要").font(.subheadline.weight(.semibold))
            if let total = model.runtimeTrendSeries?.totals {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 160), spacing: 10)], spacing: 10) {
                    MetricTile(title: "估算成本", value: costText(total.cost.estimatedCostMicros, currency: total.cost.currency), detail: total.cost.complete ? "已完成计价" : "部分请求暂无法计价", systemImage: "yensign.circle")
                    MetricTile(title: "已计价请求", value: tokenNumber(total.cost.pricedRequests), detail: "已匹配入口与模型价格", systemImage: "checkmark.circle")
                    MetricTile(title: "暂无法计价", value: tokenNumber(total.cost.unpricedRequests + total.cost.unknownAccountingRequests), detail: "缺少价格或完整用量", systemImage: "questionmark.circle")
                }
            } else {
                Text("暂无成本摘要").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    /// Projects are the primary unit users asked to compare.  Keep this table
    /// on the default board and load the complete project projection there;
    /// the separate “其他维度” board is reserved for drill-down dimensions.
    private var projectOverviewPanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let page = model.runtimeDimensionPage,
               page.kind == "project" {
                runtimeDimensionTable(page)
            } else if model.runtimeDimensionPageLoading || model.runtimeV2Loading {
                HStack {
                    ProgressView().controlSize(.small)
                    Text("正在读取项目使用情况…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else {
                EmptyStateView(title: "暂无项目使用数据", systemImage: "folder")
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
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                MetricTile(title: "输入 Token", value: tokenText(usage?.inputTokens, presence: usage?.usageFieldPresence.inputTokens), detail: "输入用量", systemImage: "arrow.down.doc", minimumHeight: overviewMetricMinimumHeight)
                MetricTile(title: "输出 Token", value: tokenText(usage?.outputTokens, presence: usage?.usageFieldPresence.outputTokens), detail: "输出用量", systemImage: "arrow.up.doc", minimumHeight: overviewMetricMinimumHeight)
                MetricTile(title: "缓存读取", value: tokenText(usage?.cacheReadInputTokens, presence: usage?.usageFieldPresence.cacheReadInputTokens), detail: "缓存读取用量", titleAccessory: "命中率 " + rateText(usage?.cacheReadTokenRate), systemImage: "externaldrive.badge.checkmark", minimumHeight: overviewMetricMinimumHeight)
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

    private var dimensionsBoard: some View {
        runtimeDimensionPanel(showsControls: true)
    }

    @ViewBuilder
    private var endpointPanel: some View {
        if let rows = analytics?.endpoints {
            SectionPanel(
                title: "入口排行",
                hint: "上游尝试按入口聚合；缓存读取下方显示缓存读取命中率，— 表示上游未返回该字段。"
            ) {
                if rows.isEmpty {
                    EmptyStateView(title: "暂无可排行的入口", systemImage: "chart.bar")
                } else {
                    Table(rows.sorted(using: endpointDetailSort), sortOrder: $endpointDetailSort) {
                            TableColumn("入口", value: \.name) { row in
                                Text(row.name).lineLimit(1).help(row.name)
                            }
                            .width(min: 150, ideal: 230, max: 320)
                            TableColumn("尝试", value: \.attempts) { row in
                                Text("\(row.attempts)").monospacedDigit()
                            }
                            .width(min: 68, ideal: 78, max: 90)
                            TableColumn("输入 Token") { row in
                                Text(tokenText(row.inputTokens, presence: row.usageFieldPresence?.inputTokens)).monospacedDigit()
                            }
                            .width(min: 82, ideal: 92, max: 110)
                            TableColumn("输出 Token") { row in
                                Text(tokenText(row.outputTokens, presence: row.usageFieldPresence?.outputTokens)).monospacedDigit()
                            }
                            .width(min: 82, ideal: 92, max: 110)
                            TableColumn("缓存读取") { row in
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(tokenText(row.cacheReadInputTokens, presence: row.usageFieldPresence?.cacheReadInputTokens)).monospacedDigit()
                                    Text("命中率 " + rateText(row.cacheReadTokenRate))
                                        .font(.caption2.monospacedDigit())
                                        .foregroundStyle(.secondary)
                                }
                            }
                            .width(min: 92, ideal: 105, max: 125)
                            TableColumn("缓存写入") { row in
                                Text(tokenText(row.cacheCreationInputTokens, presence: row.usageFieldPresence?.cacheCreationInputTokens)).monospacedDigit()
                            }
                            .width(min: 92, ideal: 105, max: 125)
                    }
                    .sumpterTableSurface()
                    .frame(minWidth: 1_020)
                    .frame(minHeight: 150)
                }
            }
        } else {
            aggregatePanel(
                title: "入口排行",
                hint: "基于最近事件窗口的上游尝试；累计见上方。usage 详情将在 SQLite analytics 可用后显示。",
                rows: analyticsRows(
                    nil,
                    fallback: UsageAggregateRow.endpointRows(from: displayRuntime.recentEvents)
                ),
                sort: $endpointSort
            )
        }
    }

    private var tokenUsagePanel: some View {
        SectionPanel(
            title: "Token 使用",
            hint: "按已记录用量汇总；没有数据时显示 —。"
        ) {
            let usage = model.runtimeTrendSeries?.totals.tokens
            VStack(alignment: .leading, spacing: 12) {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 12)], spacing: 12) {
                    MetricTile(title: "输入 Token", value: tokenText(usage?.inputTokens, presence: usage?.usageFieldPresence.inputTokens), detail: "输入用量", systemImage: "arrow.down.doc")
                    MetricTile(title: "输出 Token", value: tokenText(usage?.outputTokens, presence: usage?.usageFieldPresence.outputTokens), detail: "输出用量", systemImage: "arrow.up.doc")
                    MetricTile(title: "缓存读取", value: tokenText(usage?.cacheReadInputTokens, presence: usage?.usageFieldPresence.cacheReadInputTokens), detail: "缓存读取用量", titleAccessory: "命中率 " + rateText(usage?.cacheReadTokenRate), systemImage: "externaldrive.badge.checkmark")
                    MetricTile(title: "缓存写入", value: tokenText(usage?.cacheCreationInputTokens, presence: usage?.usageFieldPresence.cacheCreationInputTokens), systemImage: "externaldrive.badge.plus")
                }
            }
        }
    }

    private var tokenDimensionPanel: some View {
        SectionPanel(title: "入口 Token / 缓存", hint: "按当前筛选查看各入口用量。") {
            runtimeDimensionPanel(showsControls: false)
        }
    }

    /// Claude Code 请求落进「未识别项目」时的配置提示。判定在 SumpterCore
    /// (ClaudeAttributionHint,有单测);这里只展示并提供复制,绝不自动改用户的
    /// shell 配置——那超出 App 的职责边界。
    @ViewBuilder
    private var attributionHint: some View {
        if ClaudeAttributionHint.shouldPrompt(projects: attributionHintRows) {
            attributionHintCard(
                title: "Claude Code 项目归因未配置",
                message: ClaudeAttributionHint.message,
                command: ClaudeAttributionHint.command
            )
        }
        if GrokAttributionHint.shouldPrompt(projects: attributionHintRows) {
            attributionHintCard(
                title: "Grok Build 项目归因未配置",
                message: GrokAttributionHint.message,
                command: GrokAttributionHint.command
            )
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

    private var attributionHintRows: [ClaudeAttributionHint.ProjectRow] {
        (analytics?.projects ?? []).map { row in
            ClaudeAttributionHint.ProjectRow(
                name: row.name,
                projectSource: row.projectSource,
                clientKinds: row.clientKinds ?? [],
                attempts: row.attempts
            )
        }
    }

    @ViewBuilder
    private var tokenProjectTable: some View {
        if let rows = analytics?.projects, !rows.isEmpty {
            Table(rows.sorted(using: projectSort), sortOrder: $projectSort) {
                TableColumn("项目", value: \.name) { row in
                    let workspacePaths = projectWorkspacePaths(row)
                    HStack(alignment: .center, spacing: 8) {
                        Button {
                            // Legacy aggregate rows only carry the display name;
                            // keep the local drill-down on project name instead
                            // of pretending it is a stable project ID.
                            model.setRuntimeLocalProject("", projectName: row.name)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(projectDisplayName(row.name))
                                Text(projectClientKindsLabel(row.clientKinds))
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                if workspacePaths.count == 1, let path = workspacePaths.first {
                                    Text(path)
                                        .font(.caption2.monospaced())
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                        .help(path)
                                } else if workspacePaths.count > 1 {
                                    Text("\(workspacePaths.count) 条本地路径")
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                        .buttonStyle(.link)
                        .help("按项目筛选统计")
                        .accessibilityLabel("按项目筛选：\(projectDisplayName(row.name))")

                        Button {
                            locateProject(row)
                        } label: {
                            if locatingProjectName == row.name {
                                ProgressView()
                                    .controlSize(.small)
                            } else {
                                Image(systemName: "folder")
                            }
                        }
                        .buttonStyle(.borderless)
                        .frame(minWidth: 44, minHeight: 44)
                        .disabled(!projectCanLocate(row) || locatingProjectName != nil)
                        .help(projectLocatorHelp(row))
                        .accessibilityLabel(projectLocatorHelp(row))
                    }
                }
                .width(min: 220, ideal: 320)
                TableColumn("输入 Token") { row in
                    Text(tokenText(row.inputTokens, presence: row.usageFieldPresence?.inputTokens)).monospacedDigit()
                }
                .width(min: 82, ideal: 92, max: 110)
                TableColumn("输出 Token") { row in
                    Text(tokenText(row.outputTokens, presence: row.usageFieldPresence?.outputTokens)).monospacedDigit()
                }
                .width(min: 82, ideal: 92, max: 110)
                TableColumn("缓存读取") { row in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(tokenText(row.cacheReadInputTokens, presence: row.usageFieldPresence?.cacheReadInputTokens)).monospacedDigit()
                        Text("命中率 " + rateText(row.cacheReadTokenRate))
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }
                .width(min: 92, ideal: 105, max: 125)
                TableColumn("缓存写入") { row in
                    Text(tokenText(row.cacheCreationInputTokens, presence: row.usageFieldPresence?.cacheCreationInputTokens)).monospacedDigit()
                }
                .width(min: 92, ideal: 105, max: 125)
            }
            .sumpterTableSurface()
            .frame(minHeight: 150)
        } else {
            EmptyStateView(title: "所选范围暂无上游 Token usage", systemImage: "number")
        }
    }

    private func projectDisplayName(_ value: String) -> String {
        switch value {
        case "unidentified_project": "未识别项目"
        case "multiple_workspaces": "多工作区（未拆分）"
        default: value
        }
    }

    private func projectSourceLabel(_ value: String) -> String {
        switch value {
        case "workspace_local": "来源：本地项目"
        case "client_declared": "来源：客户端声明"
        case "workspace_remote_fallback": "来源：远程仓库回退"
        case "missing_workspace_metadata": "来源：未记录"
        case "multiple_workspaces": "来源：多个项目，未拆分"
        case "workspace_unidentified": "来源：未识别"
        case "mixed": "来源：混合项目来源"
        default: "来源：\(value)"
        }
    }

    private func projectCanLocate(_ row: AdminWire.RuntimeAnalytics.DimensionRow) -> Bool {
        guard row.name != "unidentified_project", row.name != "multiple_workspaces" else {
            return false
        }
        switch row.projectSource {
        case "workspace_remote_fallback", "missing_workspace_metadata", "workspace_unidentified":
            return false
        default:
            // Older daemons may omit projectSource; a normal project name is
            // still safe to search by name in Finder.
            return true
        }
    }

    /// New daemons attach sanitized workspace suffixes to each project row.
    /// Keep the event-derived fallback so an older daemon remains usable.
    private func projectWorkspacePaths(_ row: AdminWire.RuntimeAnalytics.DimensionRow) -> [String] {
        if let paths = row.workspacePaths, !paths.isEmpty {
            return paths
        }
        return ProjectFinder.workspacePaths(projectName: row.name, events: projectFinderEvents)
    }

    private func projectLocatorHelp(_ row: AdminWire.RuntimeAnalytics.DimensionRow) -> String {
        guard projectCanLocate(row) else {
            return "此项目没有可定位的本地路径"
        }
        let paths = projectWorkspacePaths(row)
        if paths.count == 1, let path = paths.first {
            return "在 Finder 中定位本地路径：\(path)"
        }
        if paths.count > 1 {
            return "在 Finder 中定位 \(paths.count) 条本地路径"
        }
        return "在 Finder 中按项目名查找本地目录（当前统计未保留对应路径）"
    }

    private var projectFinderEvents: [RuntimeEvent] {
        var seen = Set<String>()
        let paged = model.runtimePage?.events.map(\.runtimeEvent) ?? []
        return (displayRuntime.recentEvents + paged).filter { event in
            seen.insert(event.id).inserted
        }
    }

    private func locateProject(_ row: AdminWire.RuntimeAnalytics.DimensionRow) {
        guard projectCanLocate(row), locatingProjectName == nil else { return }
        let projectName = row.name
        let workspacePaths = projectWorkspacePaths(row)
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

    @ViewBuilder
    private var tokenSessionTable: some View {
        if let rows = analytics?.sessions, !rows.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("会话 Token 排行")
                    .font(.subheadline.weight(.semibold))
                Table(rows.sorted(using: sessionSort), sortOrder: $sessionSort) {
                    TableColumn("会话", value: \.name) { row in
                        Button {
                            model.setRuntimeAnalyticsFilters(sessionID: row.name)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(sessionDisplayName(row.name))
                                let context = sessionContextLabel(row)
                                if !context.isEmpty {
                                    Text(context)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(2)
                                }
                            }
                        }
                        .buttonStyle(.link)
                        .help("按会话筛选统计")
                        .accessibilityLabel("按会话筛选：\(sessionDisplayName(row.name))")
                    }
                    .width(min: 240, ideal: 360)
                    TableColumn("输入 Token") { row in
                        Text(tokenText(row.inputTokens, presence: row.usageFieldPresence?.inputTokens)).monospacedDigit()
                    }
                    .width(min: 82, ideal: 92, max: 110)
                    TableColumn("输出 Token") { row in
                        Text(tokenText(row.outputTokens, presence: row.usageFieldPresence?.outputTokens)).monospacedDigit()
                    }
                    .width(min: 82, ideal: 92, max: 110)
                    TableColumn("缓存读取") { row in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tokenText(row.cacheReadInputTokens, presence: row.usageFieldPresence?.cacheReadInputTokens)).monospacedDigit()
                            Text("命中率 " + rateText(row.cacheReadTokenRate))
                                .font(.caption2.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                    }
                    .width(min: 92, ideal: 105, max: 125)
                    TableColumn("缓存写入") { row in
                        Text(tokenText(row.cacheCreationInputTokens, presence: row.usageFieldPresence?.cacheCreationInputTokens)).monospacedDigit()
                    }
                    .width(min: 92, ideal: 105, max: 125)
                    TableColumn("操作") { row in
                        HStack(spacing: 6) {
                            Button {
                                model.exportRuntimeSession(sessionID: row.name)
                            } label: {
                                Image(systemName: "square.and.arrow.down")
                            }
                            .buttonStyle(.borderless)
                            .frame(minWidth: 44, minHeight: 44)
                            .help("导出会话 JSON")
                            Button(role: .destructive) {
                                if row.name == "unidentified_session" {
                                    unidentifiedConfirmationPhrase = ""
                                    confirmUnidentifiedSessionID = row.name
                                } else {
                                    confirmDeleteSessionID = row.name
                                }
                            } label: {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.borderless)
                            .frame(minWidth: 44, minHeight: 44)
                            .help(row.name == "unidentified_session" ? "危险操作：删除未识别会话（需输入 DELETE）" : "删除会话及统计")
                            .accessibilityLabel(row.name == "unidentified_session" ? "删除未识别会话，需要输入 DELETE 确认" : "删除会话及统计")
                        }
                    }
                    .width(min: 96, ideal: 112, max: 140)
                }
                .sumpterTableSurface()
                .frame(minHeight: 150)
            }
        }
    }

    private func clientDisplayName(_ value: String) -> String {
        switch value {
        case "claude_code": "Claude Code"
        case "codex": "Codex"
        case "grok_build": "Grok Build"
        case "openai_compat": "OpenAI 兼容客户端"
        case "unknown": "未知客户端"
        case "unrecorded_client": "旧事件（未记录）"
        default: value
        }
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
                "client_cancelled": "客户端主动取消",
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

    private func sessionContextLabel(_ row: AdminWire.RuntimeAnalytics.DimensionRow) -> String {
        let projects = (row.projects ?? []).map(projectDisplayName)
        let clients = (row.clientKinds ?? []).map(clientDisplayName)
        return [
            projects.isEmpty ? nil : "项目: \(projects.joined(separator: " + "))",
            clients.isEmpty ? nil : "客户端: \(clients.joined(separator: " + "))",
        ].compactMap { $0 }.joined(separator: " · ")
    }

    private func accountingSemanticsLabel(_ value: String?) -> String {
        switch value {
        case "subset": "缓存已包含在输入中"
        case "independent": "缓存与输入独立"
        case "mixed": "混合协议，已逐请求计算"
        default: "协议口径未知"
        }
    }

    private func accountingQualityLabel(_ value: String?) -> String {
        switch value {
        case "complete": "输入/输出完整"
        case "partial": "部分 usage 字段缺失"
        case "mixed": "完整与部分数据混合"
        default: "暂无 usage"
        }
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

    private func rateCoverageDetail(eligible: Int?, unknown: Int?) -> String {
        let known = eligible.map(tokenNumber) ?? "—"
        let unknownText = unknown.map(tokenNumber) ?? "—"
        return "可计算 \(known) · 未知 \(unknownText)"
    }

    private func formatBytes(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .memory)
    }

    private var analyticsRangePanel: some View {
        SectionPanel(title: "统计范围", hint: "请求健康与看板数据来自同一份 SQLite 稳定快照；切换范围会重新建立快照。") {
            VStack(alignment: .leading, spacing: 12) {
                if dynamicTypeSize.isAccessibilitySize {
                    VStack(alignment: .leading, spacing: 8) {
                        rangePicker
                            .pickerStyle(.menu)
                            .frame(minHeight: 44)
                        storageSummary
                    }
                } else {
                    ViewThatFits(in: .horizontal) {
                    HStack(spacing: 12) {
                        rangePicker
                            .pickerStyle(.segmented)
                            .frame(maxWidth: 420)
                        Spacer()
                        storageSummary
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        rangePicker
                            .pickerStyle(.menu)
                        storageSummary
                    }
                }
                }
            }
        }
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

    /// v3 快照控制条。看板内容在下方互斥切换，避免把所有大表纵向堆叠。
    private var runtimeV2Panel: some View {
        SectionPanel(
            title: "统计快照",
            hint: "所有看板使用同一份可重复快照；数据按统计间隔自动更新。"
        ) {
            VStack(alignment: .leading, spacing: 14) {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 10) { historyHeaderItems }
                    VStack(alignment: .leading, spacing: 8) { historyHeaderItems }
                }
                if let error = model.runtimeV2Error ?? model.runtimeHistoryError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .textSelection(.enabled)
                }
            }
        }
    }

    @ViewBuilder
    private var historyHeaderItems: some View {
        Label("按自动刷新间隔更新", systemImage: "arrow.clockwise")
            .font(.caption)
            .foregroundStyle(.secondary)
        if model.runtimeHistoryLoading || model.runtimeV2Loading {
            ProgressView().controlSize(.small)
            Text("读取中…").font(.caption).foregroundStyle(.secondary)
        }
        if let page = model.runtimeHistoryPage {
            Text("已同步 \(tokenNumber(page.totalCount)) 条")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var v3TrendSummary: some View {
        if let trend = model.runtimeTrendSeries {
            VStack(alignment: .leading, spacing: 8) {
                Text("趋势总计")
                    .font(.subheadline.weight(.semibold))
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
                    MetricTile(title: "成本", value: costText(total.cost.estimatedCostMicros, currency: total.cost.currency), detail: total.cost.complete ? "价目完整" : "部分请求未定价", systemImage: "yensign.circle")
                }
                if !trend.points.isEmpty {
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
                    .frame(minWidth: 820)
                    .frame(height: min(260, max(90, CGFloat(trend.points.count) * 26 + 34)))
                }
            }
        } else if model.runtimeV2Loading {
            HStack { ProgressView().controlSize(.small); Text("正在计算趋势…").font(.caption).foregroundStyle(.secondary) }
        }
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

    private func latencyThresholdHeader(_ thresholds: [Int]) -> String {
        guard !thresholds.isEmpty else { return "超阈值请求" }
        return thresholds
            .map { ">" + RuntimeEventPresentation.durationDisplay($0) }
            .joined(separator: " / ")
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
        if let page = model.runtimeErrorPage {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("错误聚合").font(.subheadline.weight(.semibold))
                    Spacer()
                    Text("共 \(tokenNumber(page.totalCount)) 组").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
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
                            TableColumn("失败") { group in Text([group.failureKind, group.failurePhase].compactMap { $0 }.joined(separator: " / ")).lineLimit(2).help(group.id) }.width(min: 170, ideal: 220)
                            TableColumn("入口 / 模型") { group in Text([group.endpointName ?? group.endpointID, group.model].compactMap { $0 }.joined(separator: " · ")).lineLimit(2) }.width(min: 180, ideal: 240)
                            TableColumn("次数") { group in Text(tokenNumber(group.occurrences)).monospacedDigit() }.width(min: 76, ideal: 86)
                            TableColumn("请求") { group in Text(tokenNumber(group.affectedRequests)).monospacedDigit() }.width(min: 76, ideal: 86)
                            TableColumn("会话") { group in Text(tokenNumber(group.affectedSessions)).monospacedDigit() }.width(min: 76, ideal: 86)
                            TableColumn("最近") { group in Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: group.lastSeen))).font(.caption.monospacedDigit()) }.width(min: 150, ideal: 170)
                    }
                    .sumpterTableSurface()
                    .frame(minWidth: 900)
                    .frame(height: min(240, max(90, CGFloat(page.groups.count) * 28 + 34)))
                }
            }
        }
    }

    @ViewBuilder
    private var v3DimensionTables: some View {
        if let projects = model.runtimeProjectsPage, let sessions = model.runtimeSessionsPage {
            VStack(alignment: .leading, spacing: 12) {
                dimensionTable(title: "项目（服务端分页）", page: projects, search: $model.runtimeProjectSearch, searchPrompt: "搜索项目名或 ID", onSearch: { model.loadRuntimeProjects(page: 1) }, onPage: { model.loadRuntimeProjects(page: $0) }, pageSize: model.runtimeProjectPageSize, loading: model.runtimeDimensionsLoading, onPageSizeChange: model.setRuntimeProjectPageSize, select: { model.setRuntimeV2Project($0.key, projectName: $0.name) }, isProject: true)
                dimensionTable(title: "会话（服务端分页）", page: sessions, search: $model.runtimeSessionSearch, searchPrompt: "搜索会话 ID", onSearch: { model.loadRuntimeSessions(page: 1) }, onPage: { model.loadRuntimeSessions(page: $0) }, pageSize: model.runtimeSessionPageSize, loading: model.runtimeDimensionsLoading, onPageSizeChange: model.setRuntimeSessionPageSize, select: { model.setRuntimeV2Session($0.key) }, isProject: false)
            }
        }
    }

    @ViewBuilder
    private func runtimeDimensionPanel(showsControls: Bool) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if showsControls {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 8) { runtimeDimensionControls }
                    VStack(alignment: .leading, spacing: 8) { runtimeDimensionControls }
                }
            }
            if let page = model.runtimeDimensionPage {
                runtimeDimensionTable(page)
            } else if model.runtimeDimensionPageLoading || model.runtimeV2Loading {
                HStack { ProgressView().controlSize(.small); Text("正在读取维度…").font(.caption).foregroundStyle(.secondary) }
            } else {
                EmptyStateView(title: "尚未加载维度", systemImage: "chart.bar")
            }
        }
    }

    /// Shared project/endpoint/other-dimension table.  Native Table headers
    /// provide the familiar click-to-sort affordance and expose the current
    /// direction to VoiceOver.  The selected comparator is also forwarded to
    /// the daemon so sorting applies to the complete server-side page, not
    /// only the ten rows currently visible in the window.
    @ViewBuilder
    private func runtimeDimensionTable(_ page: AdminWire.RuntimeDimensionPage) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) {
                    runtimeDimensionHeading(page)
                    Spacer(minLength: 0)
                    runtimeDimensionPager(page)
                }
                VStack(alignment: .leading, spacing: 8) {
                    runtimeDimensionHeading(page)
                    runtimeDimensionPager(page)
                }
            }
            if page.rows.isEmpty {
                EmptyStateView(title: "暂无匹配的\(runtimeDimensionLabel(page.kind))", systemImage: "chart.bar")
            } else {
                // The daemon has already sorted the complete snapshot before
                // applying LIMIT/OFFSET.  Do not sort this page again on the
                // client: doing so can reorder only the visible ten rows and
                // makes a cross-page sort appear inconsistent.
                Table(page.rows, sortOrder: $runtimeDimensionTableSort) {
                    TableColumn(runtimeDimensionLabel(page.kind), value: \.name) { row in
                        runtimeDimensionNameCell(row, kind: page.kind)
                    }
                    .width(min: 210, ideal: 280)
                    TableColumn("请求", value: \.requests) { row in
                        Text(tokenNumber(row.requests)).monospacedDigit()
                    }
                    .width(min: 76, ideal: 88)
                    TableColumn("成功率", value: \.successRateForSorting) { row in
                        Text(rateText(row.successRateForSorting >= 0 ? row.successRateForSorting : nil)).monospacedDigit()
                    }
                    .width(min: 92, ideal: 106)
                    TableColumn("失败", value: \.failures) { row in
                        Text(tokenNumber(row.failures)).monospacedDigit()
                    }
                    .width(min: 76, ideal: 88)
                    TableColumn("输入 Token", value: \.inputTokens) { row in
                        Text(dimensionTokenText(row.inputTokens, related: row.requests)).monospacedDigit()
                    }
                    .width(min: 108, ideal: 126)
                    TableColumn("输出 Token", value: \.outputTokens) { row in
                        Text(dimensionTokenText(row.outputTokens, related: row.requests)).monospacedDigit()
                    }
                    .width(min: 108, ideal: 126)
                    TableColumn("缓存读取", value: \.cacheReadInputTokens) { row in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(dimensionTokenText(row.cacheReadInputTokens, related: row.requests)).monospacedDigit()
                            Text("命中率 \(rateText(dimensionCacheTokenRate(row)))")
                                .font(.caption2.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                    }
                    .width(min: 118, ideal: 142)
                    TableColumn("缓存写入", value: \.cacheCreationInputTokens) { row in
                        Text(dimensionTokenText(row.cacheCreationInputTokens, related: row.requests)).monospacedDigit()
                    }
                    .width(min: 108, ideal: 126)
                    TableColumn("平均耗时", value: \.averageDurationForSorting) { row in
                        Text(row.averageDurationMS.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) } ?? "—").monospacedDigit()
                    }
                    .width(min: 108, ideal: 126)
                    TableColumn("最近活动", value: \.lastSeen) { row in
                        Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: row.lastSeen)))
                            .font(.caption.monospacedDigit())
                    }
                    .width(min: 150, ideal: 174)
                }
                .onChange(of: runtimeDimensionTableSort) { _, order in
                    applyRuntimeDimensionSort(order)
                }
                .sumpterTableSurface()
                .frame(maxWidth: .infinity)
                .frame(height: min(310, max(102, CGFloat(page.rows.count) * 31 + 38)))
            }
        }
    }

    private func runtimeDimensionHeading(_ page: AdminWire.RuntimeDimensionPage) -> some View {
        SumpterWrappingLayout(horizontalSpacing: 8, verticalSpacing: 4) {
            Text(page.kind == "project" ? "项目使用情况" : runtimeDimensionLabel(page.kind))
                .font(.subheadline.weight(.semibold))
            Text("共 \(tokenNumber(page.totalCount))")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            Text("列标题可切换正序 / 倒序")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }

    private func runtimeDimensionPager(_ page: AdminWire.RuntimeDimensionPage) -> some View {
        HStack(spacing: 6) {
            if model.runtimeDimensionPageLoading { ProgressView().controlSize(.small) }
            SumpterPaginationControls(
                page: page.page,
                totalPages: page.totalPages,
                hasPrevious: page.hasPrevious,
                hasNext: page.hasNext,
                pageSize: model.runtimeDimensionPageSize,
                compact: true,
                loading: model.runtimeDimensionPageLoading,
                onPageChange: { model.loadRuntimeDimensionPage(page: $0) },
                onPageSizeChange: model.setRuntimeDimensionPageSize
            )
        }
    }

    @ViewBuilder
    private func runtimeDimensionNameCell(
        _ row: AdminWire.RuntimeDimensionRow,
        kind: String
    ) -> some View {
        let label = runtimeDimensionDisplayName(row.name, kind: kind)
        let content = VStack(alignment: .leading, spacing: 2) {
            Text(label).lineLimit(1).help(row.key)
            Text(kind == "project" ? projectClientKindsLabel(row.clientKinds) : dimensionSourceDisplayName(row.source, kind: kind))
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        if kind == "project" {
            Button {
                model.setRuntimeLocalProject(row.key, projectName: row.name)
            } label: {
                content
            }
            .buttonStyle(.link)
            .help("按项目筛选统计")
            .accessibilityLabel("按项目筛选：\(label)")
        } else if kind == "session" {
            Button {
                model.setRuntimeAnalyticsFilters(sessionID: row.name)
            } label: {
                content
            }
            .buttonStyle(.link)
            .help("按会话筛选统计")
            .accessibilityLabel("按会话筛选：\(label)")
        } else {
            content
        }
    }

    private func applyRuntimeDimensionSort(
        _ order: [KeyPathComparator<AdminWire.RuntimeDimensionRow>]
    ) {
        guard let comparator = order.first else { return }
        let key = runtimeDimensionSortKey(comparator.keyPath)
        let direction = comparator.order == .reverse ? "desc" : "asc"
        model.setRuntimeDimensionSort(key, order: direction)
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
        if keyPath == \.lastSeen { return "last_seen" }
        return model.runtimeDimensionSort
    }

    private func runtimeDimensionComparator(
        sort: String,
        order: String
    ) -> KeyPathComparator<AdminWire.RuntimeDimensionRow> {
        let direction: SortOrder = order == "asc" ? .forward : .reverse
        switch sort {
        case "name": return KeyPathComparator(\.name, order: direction)
        case "requests": return KeyPathComparator(\.requests, order: direction)
        case "success_rate": return KeyPathComparator(\.successRateForSorting, order: direction)
        case "failures": return KeyPathComparator(\.failures, order: direction)
        case "input_tokens": return KeyPathComparator(\.inputTokens, order: direction)
        case "output_tokens": return KeyPathComparator(\.outputTokens, order: direction)
        case "cache_read": return KeyPathComparator(\.cacheReadInputTokens, order: direction)
        case "cache_write": return KeyPathComparator(\.cacheCreationInputTokens, order: direction)
        case "average_duration": return KeyPathComparator(\.averageDurationForSorting, order: direction)
        default: return KeyPathComparator(\.lastSeen, order: direction)
        }
    }

    @ViewBuilder
    private var runtimeDimensionControls: some View {
        Picker("维度", selection: Binding(
            get: { model.runtimeDimensionKind },
            set: { model.setRuntimeDimensionKind($0) }
        )) {
            Text("入口").tag("endpoint")
            Text("模型").tag("model")
            Text("客户端").tag("clientKind")
            Text("用途").tag("purpose")
            Text("失败类型").tag("failureKind")
            Text("失败阶段").tag("failurePhase")
            Text("协议").tag("protocol")
            Text("流终止").tag("streamTerminal")
            Text("项目").tag("project")
            Text("会话").tag("session")
        }
        .frame(minWidth: 110, idealWidth: 135)
        TextField("搜索名称或 ID", text: $model.runtimeDimensionSearch)
            .textFieldStyle(.roundedBorder)
            .frame(minWidth: 160, idealWidth: 220)
            .onSubmit { model.loadRuntimeDimensionPage(page: 1) }
        Button("搜索") { model.loadRuntimeDimensionPage(page: 1) }.controlSize(.small)
        Text("列标题可切换正序/倒序")
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: true, vertical: false)
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

    private func dimensionTable(
        title: String,
        page: AdminWire.RuntimeDimensionPage,
        search: Binding<String>,
        searchPrompt: String,
        onSearch: @escaping () -> Void,
        onPage: @escaping (Int) -> Void,
        pageSize: Int,
        loading: Bool,
        onPageSizeChange: @escaping (Int) -> Void,
        select: @escaping (AdminWire.RuntimeDimensionRow) -> Void,
        isProject: Bool
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ViewThatFits(in: .horizontal) {
                HStack {
                    dimensionTitle(title: title, totalCount: page.totalCount)
                    Spacer(minLength: 0)
                    dimensionControls(search: search, prompt: searchPrompt, onSearch: onSearch, page: page, onPage: onPage, pageSize: pageSize, loading: loading, onPageSizeChange: onPageSizeChange)
                }
                VStack(alignment: .leading, spacing: 8) {
                    dimensionTitle(title: title, totalCount: page.totalCount)
                    dimensionControls(search: search, prompt: searchPrompt, onSearch: onSearch, page: page, onPage: onPage, pageSize: pageSize, loading: loading, onPageSizeChange: onPageSizeChange)
                }
            }
            if page.rows.isEmpty {
                EmptyStateView(title: "暂无匹配的\(isProject ? "项目" : "会话")", systemImage: isProject ? "folder" : "person.2")
            } else {
                Table(page.rows) {
                        TableColumn(isProject ? "项目" : "会话") { row in
                            Button {
                                select(row)
                            } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(isProject ? projectDisplayName(row.name) : sessionDisplayName(row.name)).lineLimit(1)
                                    Text(isProject ? projectClientKindsLabel(row.clientKinds) : dimensionSourceDisplayName(row.source, kind: "session"))
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                            .buttonStyle(.link)
                            .help("使用稳定 key 筛选：\(row.key)")
                        }.width(min: 220, ideal: 320)
                        TableColumn("请求") { row in Text(tokenNumber(row.requests)).monospacedDigit() }.width(min: 80, ideal: 90)
                        TableColumn("成功") { row in Text(tokenNumber(row.successes)).monospacedDigit() }.width(min: 80, ideal: 90)
                        TableColumn("失败") { row in Text(tokenNumber(row.failures)).monospacedDigit() }.width(min: 80, ideal: 90)
                        TableColumn("Token") { row in Text(tokenNumber(row.processedTotalTokens)).monospacedDigit() }.width(min: 100, ideal: 120)
                        TableColumn("平均耗时") { row in Text(row.averageDurationMS.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) } ?? "—").monospacedDigit() }.width(min: 100, ideal: 120)
                        TableColumn("最近") { row in Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: row.lastSeen))).font(.caption.monospacedDigit()) }.width(min: 150, ideal: 170)
                }
                .sumpterTableSurface()
                .frame(minWidth: 860)
                .frame(height: min(240, max(90, CGFloat(page.rows.count) * 28 + 34)))
            }
        }
    }

    private func dimensionTitle(title: String, totalCount: Int) -> some View {
        HStack(spacing: 8) {
            Text(title).font(.subheadline.weight(.semibold))
            Text("共 \(tokenNumber(totalCount))")
                .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func dimensionControls(
        search: Binding<String>,
        prompt: String,
        onSearch: @escaping () -> Void,
        page: AdminWire.RuntimeDimensionPage,
        onPage: @escaping (Int) -> Void,
        pageSize: Int,
        loading: Bool,
        onPageSizeChange: @escaping (Int) -> Void
    ) -> some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 6) { dimensionControlItems(search: search, prompt: prompt, onSearch: onSearch, page: page, onPage: onPage, pageSize: pageSize, loading: loading, onPageSizeChange: onPageSizeChange) }
            VStack(alignment: .leading, spacing: 6) { dimensionControlItems(search: search, prompt: prompt, onSearch: onSearch, page: page, onPage: onPage, pageSize: pageSize, loading: loading, onPageSizeChange: onPageSizeChange) }
        }
    }

    @ViewBuilder
    private func dimensionControlItems(
        search: Binding<String>,
        prompt: String,
        onSearch: @escaping () -> Void,
        page: AdminWire.RuntimeDimensionPage,
        onPage: @escaping (Int) -> Void,
        pageSize: Int,
        loading: Bool,
        onPageSizeChange: @escaping (Int) -> Void
    ) -> some View {
        TextField(prompt, text: search)
            .textFieldStyle(.roundedBorder)
            .frame(minWidth: 150, idealWidth: 190)
            .onSubmit(onSearch)
        Button("搜索", action: onSearch).controlSize(.small)
        SumpterPaginationControls(
            page: page.page,
            totalPages: page.totalPages,
            hasPrevious: page.hasPrevious,
            hasNext: page.hasNext,
            pageSize: pageSize,
            compact: true,
            loading: loading,
            onPageChange: onPage,
            onPageSizeChange: onPageSizeChange
        )
    }

    private var v3ExportPanel: some View {
        SectionPanel(
            title: "运行统计导出",
            hint: "按当前筛选导出 SQLite 运行统计字段，不包含诊断正文。"
        ) {
            exportControls
            if let estimate = model.runtimeExportEstimate {
                Text("估算 \(tokenNumber(estimate.rowCount)) 行 · \(ByteCountFormatter.string(fromByteCount: Int64(clamping: estimate.estimatedBytes), countStyle: .file)) · \(estimate.privacyScope)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let error = model.runtimeExportEstimateError {
                Label(error, systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
            }
        }
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
            model.runtimeAnalyticsClientKind,
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

    private var activeRuntimeFilterLabels: [String] {
        [
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

    private var metricGrid: some View {
        SectionPanel(title: "累计", hint: "客户端请求 = 成功 + 失败 + 客户端断开/取消(499,两边都不计)。") {
            VStack(alignment: .leading, spacing: 12) {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 170), spacing: 12)], spacing: 12) {
                    MetricTile(
                        title: "范围内成功率",
                        value: analytics.map { value in
                            let completed = value.clientSuccesses + value.clientFailures + value.clientCancelled
                            return completed > 0
                                ? "\(Int((Double(value.clientSuccesses) / Double(completed) * 100).rounded()))%"
                                : "-"
                        } ?? operationalSummary.successRateText,
                        detail: analytics.map { "成功 \($0.clientSuccesses) / 失败 \($0.clientFailures) / 取消 \($0.clientCancelled) / 待定 \($0.clientPending ?? max(0, $0.clientRequests - $0.clientSuccesses - $0.clientFailures - $0.clientCancelled))" }
                            ?? "成功 \(operationalSummary.successes) / 失败 \(operationalSummary.failures) / 取消 \(operationalSummary.cancellations)",
                        systemImage: "checkmark.circle"
                    )
                    MetricTile(
                        title: "平均首字节",
                        value: analytics?.averageTTFBMS.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) }
                            ?? operationalSummary.averageTTFBMS.map { RuntimeEventPresentation.durationDisplay($0) }
                            ?? "-",
                        detail: "从发起请求到收到首个字节",
                        systemImage: "timer"
                    )
                    MetricTile(
                        title: "平均完成耗时",
                        value: analytics?.averageDurationMS.map { RuntimeEventPresentation.durationDisplay(Int($0.rounded())) }
                            ?? operationalSummary.averageDurationMS.map { RuntimeEventPresentation.durationDisplay($0) }
                            ?? "-",
                        detail: "从发起请求到完成",
                        systemImage: "clock"
                    )
                    MetricTile(
                        title: "范围内故障转移",
                        value: "\(analytics?.failovers ?? operationalSummary.failovers)",
                        detail: "\(analytics?.clientRequests ?? operationalSummary.requests) 个已决请求",
                        systemImage: "arrow.triangle.2.circlepath"
                    )
                }
                DisclosureGroup("累计计数") {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 170), spacing: 12)], spacing: 12) {
                        MetricTile(title: "客户端请求", value: "\(displayRuntime.clientRequests)", detail: "成功 \(displayRuntime.clientSuccesses)", systemImage: "person.crop.circle.badge.checkmark")
                        MetricTile(title: "客户端失败", value: "\(displayRuntime.clientFailures)", systemImage: "exclamationmark.triangle")
                        MetricTile(title: "上游尝试", value: "\(displayRuntime.upstreamAttempts)", detail: "成功 \(displayRuntime.upstreamSuccesses) / 失败 \(displayRuntime.upstreamFailures)", systemImage: "server.rack")
                        MetricTile(title: "累计故障转移", value: "\(displayRuntime.failovers)", detail: "换过入口的请求数", systemImage: "arrow.triangle.2.circlepath")
                    }
                    .padding(.top, 8)
                }
                .font(.callout.weight(.semibold))
                HStack {
                    AutoRefreshControl(
                        enabled: $model.autoRefreshEnabled,
                        intervalSeconds: $model.statisticsAutoRefreshIntervalSeconds
                    )
                    Spacer()
                    Label("按自动刷新间隔更新", systemImage: "arrow.clockwise")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
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

    @ViewBuilder
    private func aggregatePanel(
        title: String,
        hint: String? = nil,
        rows: [UsageAggregateRow],
        sort: Binding<[KeyPathComparator<UsageAggregateRow>]>
    ) -> some View {
        SectionPanel(title: title, hint: hint) {
            if rows.isEmpty {
                EmptyStateView(title: "暂无可排行的数据", systemImage: "chart.bar")
            } else {
                Table(rows.sorted(using: sort.wrappedValue), sortOrder: sort) {
                    TableColumn("名称", value: \.name) { row in
                        Text(row.name)
                            .lineLimit(1)
                    }
                    TableColumn("尝试", value: \.attempts) { row in
                        Text("\(row.attempts)")
                            .monospacedDigit()
                    }
                    TableColumn("成功率", value: \.successRate) { row in
                        Text(row.successRateText)
                            .monospacedDigit()
                    }
                    TableColumn("失败", value: \.failures) { row in
                        Text("\(row.failures)")
                            .monospacedDigit()
                    }
                    TableColumn("平均延迟", value: \.averageMS) { row in
                        Text("\(row.averageMS)ms")
                            .monospacedDigit()
                    }
                    TableColumn("输入 Token", value: \.inputTokens) { row in
                        Text(tokenText(row.inputTokens, presence: row.inputTokenPresence)).monospacedDigit()
                    }
                    TableColumn("输出 Token", value: \.outputTokens) { row in
                        Text(tokenText(row.outputTokens, presence: row.outputTokenPresence)).monospacedDigit()
                    }
                    TableColumn("缓存读取", value: \.cacheReadInputTokens) { row in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tokenText(row.cacheReadInputTokens, presence: row.cacheReadTokenPresence)).monospacedDigit()
                            Text("命中率 " + rateText(row.cacheReadTokenRate))
                                .font(.caption2.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                    }
                    TableColumn("缓存写入", value: \.cacheCreationInputTokens) { row in
                        Text(tokenText(row.cacheCreationInputTokens, presence: row.cacheCreationTokenPresence))
                            .monospacedDigit()
                    }
                }
                .sumpterTableSurface()
                .frame(height: adaptiveTableHeight(rows: rows.count, max: 220))
            }
        }
    }
}
