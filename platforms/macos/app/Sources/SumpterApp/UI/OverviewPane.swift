import SumpterCore
import SwiftUI

struct OverviewPane: View {
    @ObservedObject var model: AppModel
    @Environment(\.kekulvPalette) private var palette
    // 关闭自动刷新时冻结的画面快照;nil 表示跟随实时数据。
    // AppModel 的后台轮询(菜单栏健康点)仍在更新模型,不冻结的话表格会自己跳动,开关就形同虚设。
    @State private var frozenRuntime: RuntimeSnapshot?
    @State private var frozenHealth: ProxyHealthSummary?

    var body: some View {
        SettingsPage(title: SettingsSection.run.title, subtitle: SettingsSection.run.subtitle) {
            statusPanel
            metrics
            RecentEventsPanel(
                events: runPersistedEvents,
                liveEvents: runLiveEvents,
                hint: model.runHistoryError.map { "读取稳定事件页失败：\($0)；当前暂显示本地缓存。" }
                    ?? "客户端请求和上游尝试通过请求 ID 关联；进行中请求单独显示，不占持久事件分页名额。",
                detailedEvent: model.runtimeEventDetail?.event,
                hasMore: model.runHistoryPage?.hasNext ?? (model.runtimePage?.hasMore == true),
                currentPage: model.runHistoryPage?.page,
                totalPages: model.runHistoryPage?.totalPages,
                totalCount: model.runHistoryPage?.totalCount,
                pageLoading: model.runHistoryLoading,
                hasPreviousPage: model.runHistoryPage?.hasPrevious == true,
                onLoadFirstPage: { model.loadRunHistoryPage(1) },
                onLoadPreviousPage: {
                    guard let page = model.runHistoryPage else { return }
                    model.loadRunHistoryPage(max(1, page.page - 1))
                },
                onLoadNextPage: {
                    guard let page = model.runHistoryPage else { return }
                    model.loadRunHistoryPage(page.page + 1)
                },
                onLoadLastPage: {
                    guard let page = model.runHistoryPage else { return }
                    model.loadRunHistoryPage(max(1, page.totalPages))
                },
                onPageChange: { model.loadRunHistoryPage($0) },
                pageSize: model.runHistoryPage?.pageSize ?? model.runHistoryPageSize,
                onPageSizeChange: model.setRunHistoryPageSize,
                initialKindFilter: RuntimeEventKindFilter(rawValue: model.runHistoryKindFilter),
                onKindFilterChange: { model.setRunHistoryKindFilter($0.rawValue) },
                onSelectEvent: { model.loadRuntimeEvent(id: $0) },
                onLoadMore: {
                    guard model.runHistoryPage == nil else { return }
                    model.loadMoreRuntimeEvents()
                }
            )
            if let lastError = model.lastError, !lastError.isEmpty {
                SectionPanel(title: "最近错误") {
                    Text(lastError)
                        .foregroundStyle(.red)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .task(id: autoRefreshTaskID) {
            // 数据由 SSE 增量推送 + AppModel 兜底轮询驱动;此处只管「冻结/解冻」快照。
            // (旧版在这里另跑一个 1-30 秒轮询循环,与 AppModel 的循环叠加成重复请求。)
            guard model.autoRefreshEnabled else {
                frozenRuntime = model.runtime
                frozenHealth = model.health
                model.refreshRunHistory(resetSnapshot: model.runHistoryPage == nil)
                return
            }
            frozenRuntime = nil
            frozenHealth = nil
            model.refresh()
            model.refreshRunHistory(resetSnapshot: model.runHistoryPage == nil)
        }
    }

    /// 开关或间隔一变就重启刷新循环。
    private var autoRefreshTaskID: String {
        "\(model.autoRefreshEnabled)-\(model.runtimeAutoRefreshIntervalSeconds)"
    }

    private var displayRuntime: RuntimeSnapshot { frozenRuntime ?? model.runtime }
    private var displayHealth: ProxyHealthSummary { frozenHealth ?? model.health }

    /// 稳定历史页只包含已落盘事件；实时进行中的请求另行作为 overlay，
    /// 因此不会挤占服务端分页的名额，也不会让 totalCount 与行数失配。
    private var runPersistedEvents: [RuntimeEvent] {
        let existing = Dictionary(uniqueKeysWithValues: displayRuntime.recentEvents.map { ($0.id, $0) })
        if let page = model.runHistoryPage {
            return page.events.map { $0.mergedRuntimeEvent(with: existing[$0.id]) }
        }
        return displayRuntime.recentEvents.filter { !$0.isInFlight }
    }

    private var runLiveEvents: [RuntimeEvent] {
        let kind = model.runHistoryKindFilter
        return displayRuntime.recentEvents.filter { event in
            event.isInFlight && (kind == "all" || event.kind == kind)
        }
    }

    private var statusPanel: some View {
        SectionPanel(title: "运行状态") {
            VStack(alignment: .leading, spacing: 14) {
                // Keep the same status → controls hierarchy as Linux, but let
                // the action row move below the badges before a narrow window
                // starts compressing labels or buttons.
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 10) {
                        statusBadges
                        Spacer(minLength: 8)
                        statusActions
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        statusBadges
                        statusActions
                    }
                }
                Text(model.statusHeadline)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                if model.indicator.needsAttention {
                    Label(attentionText, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
                if let transient = model.transientMessage, !transient.isEmpty {
                    Label(transient, systemImage: "info.circle")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .transition(.opacity)
                }
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 8) {
                    InfoRow(title: "配置版本", value: "\(model.config.schemaVersion)")
                    InfoRow(title: "配置路径", value: model.configPath, copyable: true)
                    InfoRow(title: "认证状态", value: model.config.listener.hasInboundAuth ? "已启用" : "未启用")
                }
            }
        }
    }

    /// 进程异常态的行动提示(sidecar 架构独有,老单进程版没有这些故障)。
    private var attentionText: String {
        switch model.sidecarState {
        case .crashed:
            "引擎进程已退出。日志见配置目录 sumpterd.stderr.log;点「启动运行」可重新拉起。"
        case .unreachable:
            "引擎进程还在,但控制通道无响应。可先停止再启动。"
        default:
            "近期请求全部失败,检查入口配置与上游可用性。"
        }
    }

    private static func indicatorIcon(_ indicator: StatusIndicator) -> String {
        switch indicator {
        case .starting: "hourglass"
        case .crashed: "bolt.horizontal.circle.fill"
        case .unreachable: "wifi.exclamationmark"
        case .stopped: "pause.circle.fill"
        case .health(let state):
            switch state {
            case .healthy: "checkmark.circle.fill"
            case .idle: "moon.zzz"
            case .degraded: "exclamationmark.triangle.fill"
            case .down: "xmark.octagon.fill"
            case .stopped: "pause.circle.fill"
            }
        }
    }

    private var statusBadges: some View {
        let sidecarLabel = model.sidecarState.isActive ? "运行中" : "未运行"
        return SumpterWrappingLayout(horizontalSpacing: 8, verticalSpacing: 8) {
            StatusBadge(
                text: model.indicator.label,
                systemImage: Self.indicatorIcon(model.indicator),
                color: indicatorColor(model.indicator)
            )
            StatusBadge(
                text: "sidecar \(sidecarLabel)",
                systemImage: model.sidecarState.isActive ? "cpu" : "cpu.fill",
                color: model.sidecarState.isActive ? palette.success : palette.warning
            )
            StatusBadge(
                text: "\(model.config.listener.host):\(model.config.listener.port)",
                systemImage: "network",
                color: palette.info
            )
            if let storage = model.runtimeSummary?.storage {
                StatusBadge(
                    text: "SQLite \(storage.state) · 待写 \(storage.pendingEvents)",
                    systemImage: storage.state == "ready" ? "externaldrive.fill.badge.checkmark" : "externaldrive.badge.exclamationmark",
                    color: storage.state == "ready" ? palette.success : storage.state == "backpressure" ? palette.danger : palette.warning
                )
            }
        }
    }

    private var statusActions: some View {
        HStack(spacing: 10) {
            AutoRefreshControl(
                enabled: $model.autoRefreshEnabled,
                intervalSeconds: $model.runtimeAutoRefreshIntervalSeconds
            )
            Button {
                model.toggleProxy()
            } label: {
                if model.toggleInFlight {
                    // sidecar 的 spawn+握手比旧的进程内 actor 慢,过渡态必须可见。
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text(model.sidecarState.isActive ? "停止中…" : "启动中…")
                    }
                } else {
                    Label(
                        model.sidecarState.isActive ? "停止运行" : "启动运行",
                        systemImage: model.sidecarState.isActive ? "stop.fill" : "play.fill"
                    )
                }
            }
            .buttonStyle(.borderedProminent)
            .disabled(model.toggleInFlight)
        }
    }

    private func indicatorColor(_ indicator: StatusIndicator) -> Color {
        switch indicator.dotStyle {
        case .ok: palette.success
        case .neutral: .secondary
        case .warn: palette.warning
        case .fault: palette.danger
        case .quiet: .secondary
        case .starting: palette.info
        }
    }

    private var metrics: some View {
        let tokenTotals = recentTokenTotals
        return LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 12)], spacing: 12) {
            MetricTile(
                title: "输入 Token",
                value: RuntimeEventPresentation.tokenCountDisplay(tokenTotals.input),
                detail: tokenTotals.observed > 0 ? "最近事件内累计 · \(tokenTotals.observed) 个请求有用量" : "暂无可用用量",
                systemImage: "arrow.down.doc"
            )
            MetricTile(
                title: "输出 Token",
                value: RuntimeEventPresentation.tokenCountDisplay(tokenTotals.output),
                detail: tokenTotals.observed > 0 ? "最近事件内累计 · \(tokenTotals.observed) 个请求有用量" : "暂无可用用量",
                systemImage: "arrow.up.doc"
            )
            // `Provider 候选` and `Endpoints` are the same count in the flat
            // endpoint model.  Keep one metric and use its detail line to
            // preserve the routing meaning instead of presenting a duplicate
            // KPI with a different label.
            MetricTile(title: "可调度入口", value: "\(model.endpointCount)", detail: "按优先级形成 Provider 候选序列", systemImage: "arrow.triangle.branch")
            MetricTile(title: "客户端请求", value: "\(displayRuntime.clientRequests)", detail: "端到端 · 成功 \(displayRuntime.clientSuccesses) / 失败 \(displayRuntime.clientFailures)", systemImage: "arrow.down.left.and.arrow.up.right")
            MetricTile(title: "上游尝试", value: "\(displayRuntime.upstreamAttempts)", detail: "含重试 · 故障转移 \(displayRuntime.failovers)", systemImage: "point.3.connected.trianglepath.dotted")
        }
    }

    /// 运行快照没有独立的 Token 累计计数；这里按客户端事件里的 usage 汇总，
    /// 并明确标注为「最近事件内」，避免把有界快照误读成 SQLite 全量统计。
    private var recentTokenTotals: (input: Int?, output: Int?, observed: Int) {
        let usages = displayRuntime.recentEvents
            .filter { $0.kind == "client" && !$0.isInFlight && !$0.isCancelled }
            .compactMap { $0.streamTrace?.usage }
        let inputs = usages.compactMap(\.inputTokens)
        let outputs = usages.compactMap(\.outputTokens)
        return (
            inputs.isEmpty ? nil : inputs.reduce(0, +),
            outputs.isEmpty ? nil : outputs.reduce(0, +),
            usages.count
        )
    }

}
