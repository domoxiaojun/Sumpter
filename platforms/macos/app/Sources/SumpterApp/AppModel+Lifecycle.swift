import AppKit
import Foundation
import ServiceManagement
import SwiftUI
import UserNotifications
import SumpterCore

@MainActor
extension AppModel {
    /// 展示一条瞬时操作结果(成功/失败),几秒后自动消失,不占用持久健康状态。
    func flash(_ message: String) {
        transientMessage = message
        transientToken += 1
        let token = transientToken
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: 4_000_000_000)
            guard let self, self.transientToken == token else { return }
            self.transientMessage = nil
        }
    }

    func toggleProxy() {
        guard !toggleInFlight else { return }   // 防连点:spawn+握手比 actor 启动慢得多
        toggleInFlight = true
        Task {
            defer { toggleInFlight = false }
            if sidecarState.isActive {
                await stopSidecar()
                try? removeAutostartMarker()
            } else {
                await startSidecar()
                if sidecarState == .running {
                    try? writeAutostartMarker()
                }
            }
            await refreshStatus()
        }
    }

    /// spawn sumpterd 并等握手;成功后建立 admin 通道与 SSE 订阅。
    func startSidecar() async {
        guard !sidecar.isRunning else { return }
        sidecarState = .starting
        statusText = "启动中"
        // A restarted daemon has its own authoritative runtime snapshot.  Do
        // not keep rendering the previous connection's in-flight overlay while
        // the new admin channel is being established: if the old process died
        // mid-stream, that row can otherwise keep counting forever even though
        // the new daemon has already normalized it on startup.
        runtimePage = nil
        runtimeChangeSeq = 0
        runtimeEventDetails.removeAll()
        runHistoryRequestGeneration &+= 1
        runHistoryPage = nil
        runHistoryLoading = false
        runHistoryError = nil
        runtimeEventDetail = nil
        runtime.recentEvents.removeAll(where: \.isInFlight)
        do {
            let dir = try SumpterPaths.appSupportDirectory()
            let handshake = try await sidecar.start(configDir: dir)
            // token 由 sumpterd 生成/复用,读同一文件。
            controlToken = try ControlTokenStore.ensureToken(at: SumpterPaths.controlTokenURL())
            let newAdmin = AdminClient(port: handshake.adminPort, token: controlToken)
            // 握手成功不等于旧 URLSession 请求已经切换完成。先用新端口做一次
            // 轻量探针，确认 admin 真正可达后再暴露给所有诊断/统计请求。
            _ = try await newAdmin.status()
            adminConnectionGeneration &+= 1
            admin = newAdmin
            engineGeneration = handshake.generation
            sidecarState = .running
            isProxyRunning = true
            statusText = "运行中"
            lastAppliedListener = config.listener
            subscribeAdminEvents(connectionGeneration: adminConnectionGeneration)
            // 新 admin 端口确认可达后立即刷新轻量捕获索引，避免监听重启后诊断页
            // 还停留在旧连接的错误或空状态，正文仍按需加载。
            refreshDiagnosticCapture()
        } catch {
            // 握手后的 admin 探针失败时 sidecar 可能已经启动；必须回收它，
            // 否则下一次启动会被 isRunning 拦截而继续复用失效端口。
            await stopSidecar()
            sidecarState = .stopped
            lastError = "\(error)"
            statusText = "启动失败"
            flash("引擎启动失败")
        }
    }

    func stopSidecar() async {
        // 先失效连接和所有 latest-wins 代次，再等待进程退出。MainActor 在 await
        // 期间可重入；如果晚清空 admin，用户此时点击诊断会继续打到旧随机端口。
        adminConnectionGeneration &+= 1
        refreshRequestGeneration &+= 1
        analyticsRequestGeneration &+= 1
        detailRequestGeneration &+= 1
        lastRuntimeAnalyticsRefreshAt = nil
        admin = nil
        eventsTask?.cancel()
        eventsTask = nil
        countersRefreshTask?.cancel()
        countersRefreshTask = nil
        runHistoryAutoRefreshTask?.cancel()
        runHistoryAutoRefreshTask = nil
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureTask = nil
        diagnosticCaptureDetailTask?.cancel()
        diagnosticCaptureDetailTask = nil
        diagnosticCaptureExportTask?.cancel()
        diagnosticCaptureExportRequestGeneration &+= 1
        diagnosticCaptureExportTask = nil
        runtimeRequestChainTask?.cancel()
        runtimeRequestChainTask = nil
        diagnosticCaptureRequestGeneration &+= 1
        diagnosticCaptureDetailRequestGeneration &+= 1
        diagnosticCapture = nil
        diagnosticCaptureDetail = nil
        diagnosticCaptureError = nil
        diagnosticCaptureDetailError = nil
        diagnosticCaptureBusy = false
        diagnosticCaptureDetailBusy = false
        diagnosticCaptureExportBusy = false
        runtimeV2RequestGeneration &+= 1
        runHistoryRequestGeneration &+= 1
        runtimeErrorPageRequestGeneration &+= 1
        runtimeDimensionRequestGeneration &+= 1
        runtimeRequestChainRequestGeneration &+= 1
        runtimeMaintenanceRequestGeneration &+= 1
        runtimeExportRequestGeneration &+= 1
        runtimeExportEstimateRequestGeneration &+= 1
        runtimeV2Loading = false
        runHistoryLoading = false
        runHistoryPage = nil
        runHistoryError = nil
        runtimeHistoryLoading = false
        runtimeRequestChainLoading = false
        runtimeErrorPageLoading = false
        runtimeDimensionsLoading = false
        runtimeExportBusy = false
        clearRuntimeV2Snapshot()
        runtimeHistoryError = nil
        runtimeV2Error = nil
        runtimeExportEstimateError = nil
        isProxyRunning = false
        engineGeneration = nil
        await sidecar.stop()
        sidecarState = .stopped
        statusText = "已停止"
    }

    /// 订阅 sumpterd 的 SSE:运行事件增量上屏、通知投递、外部配置变更对账。
    func subscribeAdminEvents(connectionGeneration: Int) {
        eventsTask?.cancel()
        guard let admin else { return }
        eventsTask = Task { [weak self] in
            var retryNanoseconds: UInt64 = 1_000_000_000
            while !Task.isCancelled {
                do {
                    guard let model = self else { return }
                    guard model.adminConnectionGeneration == connectionGeneration,
                          model.admin != nil else { return }
                    await model.reconcileRuntimeChanges(using: admin)
                    for try await event in admin.events() {
                        guard let model = self else { return }
                        guard model.adminConnectionGeneration == connectionGeneration,
                              !Task.isCancelled else { return }
                        await model.handleAdminEvent(event)
                        retryNanoseconds = 1_000_000_000
                    }
                } catch {
                    if Task.isCancelled { return }
                }
                try? await Task.sleep(nanoseconds: retryNanoseconds)
                retryNanoseconds = min(retryNanoseconds * 2, 15_000_000_000)
            }
        }
    }

    func reconcileRuntimeChanges(using admin: AdminClient) async {
        guard autoRefreshEnabled else { return }
        guard runtimeChangeSeq > 0 else {
            // A zero cursor is explicitly non-authoritative (old daemon,
            // reset, or a page without change metadata). Re-read the latest
            // page instead of pretending SSE is a complete source of truth.
            if runtimePage != nil {
                await refreshStatus(loadLatestEvents: true)
            }
            return
        }
        do {
            var cursor = runtimeChangeSeq
            while true {
                let page = try await admin.runtimeEvents(afterChangeSeq: cursor, limit: 200)
                guard page.cursorValid != false,
                      page.resetGeneration == nil || page.resetGeneration == runtimeSummary?.resetGeneration else {
                    await refreshStatus(loadLatestEvents: true)
                    return
                }
                for item in page.events {
                    applyRuntimeListItem(item, resetGeneration: page.resetGeneration)
                }
                let nextCursor = page.events.map(\.changeSeq).max() ?? cursor
                guard page.hasMore else { return }
                guard !page.events.isEmpty, nextCursor > cursor else {
                    await refreshStatus(loadLatestEvents: true)
                    return
                }
                cursor = nextCursor
            }
        } catch {
            await refreshStatus(loadLatestEvents: true)
        }
    }

    /// 事件即时并入本地快照(与引擎同口径:同 id 原地更新、按类各留 200 条),
    /// 计数则防抖合并拉取——一次长流会推很多 delta 事件,不该每条都打一次 admin。
    func applyRuntimeEvent(_ event: RuntimeEvent) {
        var events = runtime.recentEvents
        if let index = events.firstIndex(where: { $0.id == event.id }) {
            events[index] = event
        } else {
            events.insert(event, at: 0)
            events = RuntimeEvent.trimmed(events, perKindLimit: 200)
        }
        runtime.recentEvents = events
        let evaluatedHealth = ProxyHealthEvaluator.evaluate(
            events: events,
            isRunning: sidecarState == .running
        )
        if evaluatedHealth != health {
            health = evaluatedHealth
        }
        scheduleCountersRefresh()
    }

    func applyRuntimeListItem(
        _ item: AdminWire.RuntimeEventListItem,
        resetGeneration: Int? = nil
    ) {
        runtimeChangeSeq = max(runtimeChangeSeq, item.changeSeq)
        // Statistics are served from a stable SQLite snapshot and do not need
        // the run page's per-event overlay. Avoid publishing the high-rate
        // runtime list while this pane is visible; the next page transition
        // (or window show) performs one authoritative refresh for the run UI.
        guard !statisticsVisible else { return }
        var items = runtimePage?.events ?? []
        if let index = items.firstIndex(where: { $0.id == item.id }) {
            // SSE reconnects and a paged response can deliver an older change.
            // Ignore it everywhere, not only in the compact page, otherwise it
            // could still roll back the full in-memory RuntimeEvent below.
            guard item.changeSeq >= items[index].changeSeq else { return }
            items[index] = item
        } else {
            items.append(item)
        }
        items.sort { $0.seq > $1.seq }
        let retainedLimit = max(runtimePage?.events.count ?? 50, 50)
        if items.count > retainedLimit {
            items = Array(items.prefix(retainedLimit))
        }
        runtimePage = AdminWire.RuntimeEventPage(
            events: items,
            hasMore: runtimePage?.hasMore ?? false,
            resetGeneration: resetGeneration ?? runtimePage?.resetGeneration,
            cursorValid: true
        )
        // The list endpoint intentionally returns a compact projection. Merge it
        // into the existing full event so an SSE/list refresh cannot erase usage,
        // tool calls, streamTrace or Codex metadata already loaded for this ID.
        let existing = runtime.recentEvents.first(where: { $0.id == item.id })
        applyRuntimeEvent(item.mergedRuntimeEvent(with: existing))
        // 进行中的 delta 只更新实时 overlay；进入终态后自动重建第一页。
        // 旧 daemon 若没有 phase 也必须走这条兼容路径。
        if item.phase != .inFlight {
            scheduleRunHistoryAutoRefresh()
        }
    }

    /// 自动把运行页第一页推进到最新稳定快照。用户正在查看更早页时不
    /// 强行跳页；回到第一页的分页动作会主动建立新快照。
    func scheduleRunHistoryAutoRefresh() {
        guard autoRefreshEnabled, runHistoryPage?.page == 1,
              runHistoryAutoRefreshTask == nil else { return }
        runHistoryAutoRefreshTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 220_000_000)
            guard let self else { return }
            self.runHistoryAutoRefreshTask = nil
            guard !Task.isCancelled, self.autoRefreshEnabled,
                  self.runHistoryPage?.page == 1 else { return }
            self.refreshRunHistory(resetSnapshot: true)
        }
    }

    func scheduleCountersRefresh() {
        guard countersRefreshTask == nil else { return }
        countersRefreshTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            guard let self else { return }
            self.countersRefreshTask = nil
            await self.refreshStatus()
        }
    }

    func handleAdminEvent(_ event: AdminWire.Event) async {
        switch event {
        case .notify(let clientKind, let title, let message, _, let kind, let category, _, _, let sessionID, let cwd):
            guard notificationsEnabled else { return }
            if clientKind == "codex" {
                guard codexNotificationsEnabled else { return }
            } else if clientKind == "grok_build" {
                guard grokNotificationsEnabled else { return }
            } else {
                guard claudeNotificationsEnabled else { return }
            }
            if clientKind == "codex" {
                // A real SSE delivery is the only reliable evidence that the
                // Codex `/hooks` trust step has completed.
                CodexNotificationHooks.markVerified()
                codexNotificationHookStatus = .verified
            }
            guard shouldDeliverNotification(category: category) else { return }
            if clientKind == "codex" {
                await deliverNotification(
                    title: title,
                    message: message,
                    kind: kind,
                    category: category,
                    sessionID: sessionID,
                    // Codex notifications stay fully fixed and do not expose
                    // even the workspace basename as a subtitle.
                    cwd: nil,
                    clientKind: "codex"
                )
            } else if clientKind == "grok_build" {
                await deliverNotification(
                    title: title,
                    message: message,
                    kind: kind,
                    category: category,
                    sessionID: sessionID,
                    cwd: cwd,
                    clientKind: "grok_build"
                )
            } else {
                await deliverNotification(
                    title: title,
                    message: message,
                    kind: kind,
                    category: category,
                    sessionID: sessionID,
                    cwd: cwd,
                    clientKind: "claude_code"
                )
            }
        case .configReloaded(let generation):
            // 外部 /__reload 或 CLI 改动:sumpterd 已自主读盘,UI 重读磁盘对账内存副本。
            engineGeneration = generation
            let previousListener = config.listener
            var listenerChanged = false
            if let store, let result = try? store.loadWithMigration() {
                presentMigrationNoticeIfNeeded(result.migrationNotice)
                let normalized = result.config.normalizedBuiltInFeatureRules()
                if normalized != config {
                    listenerChanged = normalized.listener != previousListener
                    config = normalized
                    flash("配置已在外部变更,已重新载入")
                }
            }
            if listenerChanged {
                // 外部改 host/port 时 daemon 的热 reload 不会重绑 proxy listener，
                // 必须走与安全页相同的完整重启和新 admin 握手。
                do {
                    try await pushConfigToSidecar()
                } catch {
                    lastError = "监听配置重启失败：\(error)"
                    flash("监听配置重启失败")
                }
            } else {
                await refreshStatus()
            }
        case .migrationNotice(let notice):
            presentMigrationNoticeIfNeeded(notice)
        case .statsReset:
            guard autoRefreshEnabled else { return }
            runtimeChangeSeq = 0
            runtimeEventDetails.removeAll()
            runtimeEventDetail = nil
            runtimePage = nil
            runHistoryRequestGeneration &+= 1
            runHistoryPage = nil
            runHistoryError = nil
            runtime.recentEvents = []
            await refreshStatus(loadLatestEvents: true)
        case .runtimeChange(let change):
            guard autoRefreshEnabled else { return }
            guard change.seq > 0, change.changeSeq > 0 else {
                await refreshStatus(loadLatestEvents: true)
                return
            }
            applyRuntimeListItem(AdminWire.RuntimeEventListItem(change: change))
            if change.changeSeq >= (runtimeEventDetails[change.event.id]?.changeSeq ?? 0) {
                runtimeEventDetails[change.event.id] = change
                if runtimeEventDetails.count > 200, let oldest = runtimeEventDetails.min(by: { $0.value.changeSeq < $1.value.changeSeq })?.key { runtimeEventDetails.removeValue(forKey: oldest) }
                if runtimeEventDetail?.event.id == change.event.id { runtimeEventDetail = change }
            }
        }
    }

    /// 用户侧只有一个通知总开关；Claude/Codex/Grok 的配置文件仍由各自安全编辑器
    /// 维护，但一次操作会同步安装或移除三边的通知 Hook。
    var notificationsEnabled: Bool {
        !claudeNotificationArguments.isEmpty
            || !codexNotificationArguments.isEmpty
            || grokNotificationsEnabled
    }

    func refresh() {
        Task { await refreshStatus(loadLatestEvents: true) }
    }

    /// UsagePane 在进入/离开导航详情时调用。统计聚合不是 liveness 数据，
    /// 因此不让后台 status 轮询在其它页面预取它。
    func setStatisticsVisible(_ visible: Bool) {
        guard statisticsVisible != visible else { return }
        statisticsVisible = visible
        // UsagePane immediately calls refreshStatisticsIfNeeded after marking
        // the page visible. Keep this setter about lifecycle/generation only;
        // loading facets here would duplicate the first snapshot request and
        // make entering Statistics pay for two identical SQLite scans.
        if !visible {
            // 让离开页面后返回的旧响应失效，但不清空已显示快照，回到页面
            // 时可以先绘制旧数据再按需更新。
            analyticsRequestGeneration &+= 1
            runtimeAnalyticsLoading = false
            runtimeV2RequestGeneration &+= 1
            runtimeV2Loading = false
        }
    }

    /// 设置当前统计看板。首屏只加载基础页/趋势/存储/价格；错误与项目/会话
}
