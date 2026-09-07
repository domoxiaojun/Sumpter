import AppKit
import Foundation
import ServiceManagement
import SwiftUI
import UserNotifications
import SumpterCore

@MainActor
extension AppModel {
    func bootstrap() async {
        do {
            let url = try SumpterPaths.configURL()
            configPath = url.path
            tightenSensitiveFilePermissions()
            let store = ConfigStore(url: url)
            self.store = store
            if FileManager.default.fileExists(atPath: url.path) {
                let result = try store.loadWithMigration()
                presentMigrationNoticeIfNeeded(result.migrationNotice)
                let loaded = result.config
                config = loaded.normalizedBuiltInFeatureRules()
                if config != loaded {
                    try store.save(config)
                }
            } else {
                config = .bootstrap
                try store.save(config)
            }
            controlToken = try ControlTokenStore.ensureToken(at: try SumpterPaths.controlTokenURL())
            notificationAuthorizationStatus = await NativeNotifier.shared.authorizationStatus()
            sidecar.onUnexpectedExit = { [weak self] code in
                guard let self else { return }
                self.eventsTask?.cancel()
                self.eventsTask = nil
                self.countersRefreshTask?.cancel()
                self.countersRefreshTask = nil
                self.admin = nil
                self.sidecarState = .crashed(code)
                self.isProxyRunning = false
                self.engineGeneration = nil
                self.statusText = "引擎异常退出"
                self.lastError = "sumpterd 异常退出,code=\(code)"
                self.flash("代理引擎异常退出")
                self.health = ProxyHealthEvaluator.evaluate(
                    events: self.runtime.recentEvents,
                    isRunning: false
                )
            }
            refreshNotificationHookState()
            // hooks 已启用则启动时重写脚本:脚本模板升级(如 v2 转发 stdin)随 app
            // 更新自动生效,不用等用户碰通知开关或改端口。
            try? ClaudeNotificationHooks.rewriteScriptIfEnabled(port: config.listener.port)
            if codexNotificationsEnabled {
                try? CodexNotificationHooks.rewriteScriptIfEnabled(port: config.listener.port)
            }
            if grokNotificationsEnabled {
                try? GrokNotificationHooks.rewriteScriptIfEnabled(port: config.listener.port)
            } else if (claudeNotificationsEnabled || codexNotificationsEnabled)
                && !GrokNotificationHooks.wasRemovedByUser()
            {
                // 升级前总开关已开、还没有 Grok hook：补装一次。用户手动移除后不再强行写回。
                try? GrokNotificationHooks.setEnabled(true, port: config.listener.port)
            }
            refreshNotificationHookState()
            refreshLoginItemStatus()
            if FileManager.default.fileExists(atPath: try SumpterPaths.autostartURL().path) {
                await startSidecar()
            }
            await refreshStatus()
        } catch {
            lastError = "\(error)"
            statusText = "初始化失败"
        }
    }

    func persistConfigAndRefresh() async throws {
        let predecessor = configPersistenceTail
        let operation = Task { @MainActor [weak self] in
            if let predecessor {
                await predecessor.value
            }
            guard let self else { return }
            if self.store == nil {
                let url = try SumpterPaths.configURL()
                self.store = ConfigStore(url: url)
                self.configPath = url.path
            }
            self.config.normalizeBuiltInFeatureRules()
            if let store = self.store {
                let snapshot = self.config
                // 磁盘写挪出 MainActor,避免保存时界面卡顿。
                do {
                    try await Task.detached(priority: .utility) {
                        try store.save(snapshot)
                    }.value
                } catch {
                    throw ConfigSaveError(code: "config_write_failed", reason: error.localizedDescription)
                }
            }
            do {
                try await self.pushConfigToSidecar()
            } catch {
                let adminError = error as? AdminClient.AdminError
                throw ConfigSaveError(code: adminError?.serverCode ?? "reload_failed", reason: error.localizedDescription)
            }
            await self.refreshStatus()
        }
        configPersistenceTail = Task { _ = try? await operation.value }
        try await operation.value
    }

    func deliverNotification(
        title: String,
        message: String,
        kind: String? = nil,
        category: String? = nil,
        sessionID: String? = nil,
        cwd: String? = nil,
        clientKind: String = "claude_code"
    ) async {
        do {
            // 来源前缀保证 Claude 与 Codex 即使复用 session id 也不会混组。
            let thread = "\(clientKind):\(sessionID ?? category ?? kind ?? "sumpter")"
            let subtitle = cwd.map { URL(fileURLWithPath: $0).lastPathComponent } ?? ""
            notificationAuthorizationStatus = try await NativeNotifier.shared.deliver(
                title: title,
                message: message,
                subtitle: subtitle,
                threadIdentifier: thread,
                soundPreference: notificationSoundPreference
            )
            notificationError = nil
        } catch {
            notificationAuthorizationStatus = await NativeNotifier.shared.authorizationStatus()
            notificationError = "\(error)"
            lastError = "\(error)"
        }
    }

    func shouldDeliverNotification(category: String?) -> Bool {
        guard let category else { return true }
        return notificationCategoryEnabled(category)
    }

    func notificationCategoryEnabled(_ category: String) -> Bool {
        switch category {
        case "action_required": return actionNotificationsEnabled
        case "status": return statusNotificationsEnabled
        case "turn_completed": return turnCompletionNotificationsEnabled
        case "subtask_completed": return subtaskNotificationsEnabled
        case "turn_failed": return failureNotificationsEnabled
        default: return true
        }
    }

    func notificationCategoryKey(_ category: String) -> String {
        switch category {
        case "action_required": return "notificationActionRequiredEnabled"
        case "status": return "notificationStatusEnabled"
        case "turn_completed": return "notificationTurnCompletedEnabled"
        case "subtask_completed": return "notificationSubtaskCompletedEnabled"
        case "turn_failed": return "notificationTurnFailedEnabled"
        default: return "notificationUnknownEnabled"
        }
    }

    /// 兜底刷新:周期轮询只拉 status/summary；统计页自己的 v3 快照独立加载。
    /// 首次加载、手动刷新、reset
    /// 或增量游标失效时才替换最新事件页，避免覆盖用户已加载的历史分页。
    /// 每类请求独立处理错误：analytics/storage 失败不能伪装成 daemon 不可达。
    func refreshStatus(
        loadLatestEvents: Bool = false,
        reconcileEvents: Bool = false
    ) async {
        refreshRequestGeneration &+= 1
        let generation = refreshRequestGeneration
        guard sidecar.isRunning, let admin else {
            if isProxyRunning { isProxyRunning = false }
            if case .crashed = sidecarState {
                // 崩溃态由 onUnexpectedExit 设置,这里不覆盖。
            } else if sidecarState != .stopped {
                sidecarState = .stopped
            }
            if statusText != "启动失败" && statusText != "引擎异常退出" && statusText != "初始化失败" {
                if statusText != "已停止" { statusText = "已停止" }
            }
            let summary = ProxyHealthEvaluator.evaluate(events: runtime.recentEvents, isRunning: false)
            if summary != health { health = summary }
            return
        }
        let status: AdminWire.Status
        do {
            status = try await admin.status()
        } catch {
            guard generation == refreshRequestGeneration else { return }
            // Only the liveness request controls the unreachable state.
            if sidecarState != .unreachable { sidecarState = .unreachable }
            if statusText != "引擎无响应" { statusText = "引擎无响应" }
            let evaluated = ProxyHealthEvaluator.evaluate(events: runtime.recentEvents, isRunning: false)
            if evaluated != health { health = evaluated }
            return
        }
        guard generation == refreshRequestGeneration else { return }
        guard status.runtimeApiVersion == nil || status.runtimeApiVersion == Self.runtimeAPIVersion else {
            lastError = "daemon 运行统计 API 版本为 v\(status.runtimeApiVersion ?? -1)，App 需要 v\(Self.runtimeAPIVersion)。请同步升级。"
            return
        }

        var summary: AdminWire.RuntimeSummary?
        do {
            let value = try await admin.runtimeSummary()
            guard value.apiVersion == Self.runtimeAPIVersion else {
                throw AppModelError.invalidInput(
                    "运行统计 API 版本不匹配：需要 v\(Self.runtimeAPIVersion)，当前为 v\(value.apiVersion)。请同步升级。"
                )
            }
            summary = value
            runtimeSummaryError = nil
        } catch {
            guard generation == refreshRequestGeneration else { return }
            runtimeSummaryError = "\(error)"
        }

        var page: AdminWire.RuntimeEventPage?
        if loadLatestEvents || runtimePage == nil {
            do {
                page = try await admin.runtimeEvents(limit: 10)
                runtimeEventsError = nil
            } catch {
                guard generation == refreshRequestGeneration else { return }
                runtimeEventsError = "\(error)"
            }
        }
        guard generation == refreshRequestGeneration else { return }

        if let summary {
            let resetGenerationChanged = runtimeSummary.map {
                $0.resetGeneration != summary.resetGeneration
            } ?? false
            if resetGenerationChanged, page == nil {
                page = try? await admin.runtimeEvents(limit: 10)
                guard generation == refreshRequestGeneration else { return }
            }
            if resetGenerationChanged {
                runtimeChangeSeq = 0
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
            }
            let counters = summary.counters
            let existing = Dictionary(uniqueKeysWithValues: runtime.recentEvents.map { ($0.id, $0) })
            let mergedEvents = page?.events.map { $0.mergedRuntimeEvent(with: existing[$0.id]) } ?? runtime.recentEvents
            let snapshot = RuntimeSnapshot(
                clientRequests: counters.clientRequests,
                clientSuccesses: counters.clientSuccesses,
                clientFailures: counters.clientFailures,
                upstreamAttempts: counters.upstreamAttempts,
                upstreamSuccesses: counters.upstreamSuccesses,
                upstreamFailures: counters.upstreamFailures,
                failovers: counters.failovers,
                recentEvents: mergedEvents
            )
            if runtime != snapshot { runtime = snapshot }
            if runtimeSummary != summary { runtimeSummary = summary }
            if let page, runtimePage != page { runtimePage = page }
            if let page {
                let pageCursor = page.events.map(\.changeSeq).max() ?? 0
                runtimeChangeSeq = resetGenerationChanged ? pageCursor : max(runtimeChangeSeq, pageCursor)
            }
        }
        if !isProxyRunning { isProxyRunning = true }
        if sidecarState != .running { sidecarState = .running }
        if endpointCount != status.endpoints { endpointCount = status.endpoints }
        if engineGeneration != status.generation { engineGeneration = status.generation }
        // status.lastError is the daemon's own operational error only.
        if lastError != status.lastError { lastError = status.lastError }
        if statusText != "运行中" { statusText = "运行中" }
        let evaluatedHealth = ProxyHealthEvaluator.evaluate(events: runtime.recentEvents, isRunning: true)
        if evaluatedHealth != health { health = evaluatedHealth }

        if reconcileEvents && autoRefreshEnabled && !loadLatestEvents {
            await reconcileRuntimeChanges(using: admin)
        }
    }

    /// best-effort:把配置目录里可能含明文 key 的文件(含历史备份)收紧到 0600。
    func tightenSensitiveFilePermissions() {
        guard let directory = try? SumpterPaths.appSupportDirectory() else {
            return
        }
        let manager = FileManager.default
        guard let entries = try? manager.contentsOfDirectory(atPath: directory.path) else {
            return
        }
        for name in entries {
            let isSensitive = name == "keys.json"
                || name.hasPrefix("app-config")
                || name.hasPrefix("keys")
            guard isSensitive, name.hasSuffix(".json") else {
                continue
            }
            let path = directory.appendingPathComponent(name).path
            try? manager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path)
        }
    }

    func writeAutostartMarker() throws {
        let url = try SumpterPaths.autostartURL()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try "1".write(to: url, atomically: true, encoding: .utf8)
    }

    func removeAutostartMarker() throws {
        let url = try SumpterPaths.autostartURL()
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
    }

    func uniqueEndpointID(_ name: String) -> String {
        let base = name
            .lowercased()
            .map { char in
                char.isLetter || char.isNumber ? char : "-"
            }
            .reduce(into: "") { $0.append($1) }
            .split(separator: "-")
            .joined(separator: "-")
        let prefix = base.isEmpty ? "provider" : base
        let existing = Set(config.endpoints.map(\.id))
        if !existing.contains(prefix) {
            return prefix
        }
        var index = 2
        while existing.contains("\(prefix)-\(index)") {
            index += 1
        }
        return "\(prefix)-\(index)"
    }

    func modelCatalogTimestamp() -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter.string(from: Date())
    }

    /// Admin API 返回 Unix 秒数字符串；配置目录在 macOS UI 中沿用可读的
    /// 本地时间格式，避免把 `1786233600` 直接展示给用户。
    func modelCatalogTimestamp(_ raw: String) -> String {
        guard let seconds = Double(raw), seconds.isFinite, seconds > 0 else {
            return raw.isEmpty ? modelCatalogTimestamp() : raw
        }
        let date = Date(timeIntervalSince1970: seconds)
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter.string(from: date)
    }


    func nilIfBlank(_ text: String) -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
