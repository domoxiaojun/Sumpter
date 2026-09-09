import AppKit
import Foundation
import ServiceManagement
import SwiftUI
import UserNotifications
import SumpterCore

@MainActor
extension AppModel {
    func refreshNow() async {
        await refreshStatus(loadLatestEvents: true)
    }

    /// 退出前先停 sidecar(其 SIGTERM 路径会把统计落盘),避免防抖窗口内的最近事件丢失。
    func shutdownAndQuit() {
        Task {
            await stopSidecar()
            await MainActor.run {
                NSApplication.shared.terminate(nil)
            }
        }
    }

    func saveConfig() {
        Task {
            do {
                try await persistConfigAndRefresh()
            } catch {
                lastError = "\(error)"
                flash("保存失败")
            }
        }
    }

    /// 从磁盘重新加载 `config.json` 并推给引擎(读 `SumpterPaths.configURL()`;
    /// 旧 keys.json 早已不参与加载)。
    /// 用于手动编辑配置文件后无需重启即可生效(app 不监听文件变化)。
    /// 也可由控制端点 `POST /__reload` 经 `setReloadHandler` 触发。
    func reloadConfigFromDisk() {
        Task {
            do {
                try await performReloadConfigFromDisk()
                flash("已从磁盘重新加载配置")
            } catch {
                lastError = "\(error)"
                flash("重新加载失败")
            }
        }
    }

    /// 可 await 的磁盘重载实现,供菜单使用(外部 `/__reload` 由 sumpterd 自主处理并经 SSE 对账)。
    func performReloadConfigFromDisk() async throws {
        let url = try SumpterPaths.configURL()
        let store = self.store ?? ConfigStore(url: url)
        self.store = store
        configPath = url.path
        let result = try store.loadWithMigration()
        presentMigrationNoticeIfNeeded(result.migrationNotice)
        let raw = result.config
        let loaded = raw.normalizedBuiltInFeatureRules()
        if loaded != raw {
            try store.save(loaded)
        }
        config = loaded
        try await pushConfigToSidecar()
        await refreshStatus()
    }

    /// 配置已落盘后让 sumpterd 生效:监听地址变了要重启进程(热 reload 不重绑端口),
    /// 否则 admin reload 热加载并拿 generation/warnings 回执。
    func pushConfigToSidecar() async throws {
        guard sidecar.isRunning else { return }
        if let applied = lastAppliedListener, applied != config.listener {
            // 监听地址/端口变了:热 reload 不重绑端口,必须重启进程。
            await stopSidecar()
            await startSidecar()
            flash("监听配置已变更,引擎已重启")
            return
        }
        guard let admin else { return }
        let ack = try await admin.reload()
        lastAppliedListener = config.listener
        // 对账:generation 是 sumpterd 对刚读到的配置算的代号,对不上说明引擎没跟上。
        engineGeneration = ack.generation
        // reload 回执仍完整解码以兼容 daemon wire，但不再在 App 内展示
        // 配置风险监测；这里只反馈 reload 本身。
        flash("配置已生效")
    }

    func resetRuntimeStats() {
        Task {
            guard let admin else { return }
            do {
                try await admin.resetRuntime()
                runtimeChangeSeq = 0
                runtimeEventDetails.removeAll()
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
                runtime = RuntimeSnapshot()
                clearRuntimeV2Snapshot()
                runtimeV2RequestGeneration &+= 1
                runtimeErrorPageRequestGeneration &+= 1
                runtimeDimensionRequestGeneration &+= 1
                resetRuntimeLocalDrillDownState()
                flash("SQLite 新统计已清空")
            } catch {
                lastError = "\(error)"
                flash("重置统计失败")
            }
            await refreshStatus(loadLatestEvents: true)
        }
    }

    func recreateRuntimeStats() {
        Task {
            guard let admin else { return }
            do {
                try await admin.recreateRuntime()
                runtimeChangeSeq = 0
                runtimeEventDetails.removeAll()
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
                runtime = RuntimeSnapshot()
                clearRuntimeV2Snapshot()
                runtimeV2RequestGeneration &+= 1
                runtimeErrorPageRequestGeneration &+= 1
                runtimeDimensionRequestGeneration &+= 1
                runtimeMaintenanceRequestGeneration &+= 1
                resetRuntimeLocalDrillDownState()
                flash("已重置并新建 SQLite 数据库")
            } catch {
                lastError = "\(error)"
                flash("新建数据库失败")
            }
            await refreshStatus(loadLatestEvents: true)
        }
    }

    func previewRuntimeCleanup(olderThan: Double) async throws -> AdminWire.RuntimeCleanupPreview {
        guard let admin else { throw AppModelError.invalidInput("管理连接尚未就绪") }
        return try await admin.previewRuntimeCleanup(olderThan: olderThan)
    }

    func cleanupRuntime(olderThan: Double) async throws -> AdminWire.RuntimeCleanupMutation {
        guard let admin else { throw AppModelError.invalidInput("管理连接尚未就绪") }
        let mutation = try await admin.cleanupRuntime(olderThan: olderThan)
        runtimeChangeSeq = 0
        runtimeEventDetails.removeAll()
        runtimeEventDetail = nil
        runtimePage = nil
        runHistoryRequestGeneration &+= 1
        runHistoryPage = nil
        runHistoryError = nil
        clearRuntimeV2Snapshot()
        runtimeV2RequestGeneration &+= 1
        runtimeErrorPageRequestGeneration &+= 1
        runtimeDimensionRequestGeneration &+= 1
        resetRuntimeLocalDrillDownState()
        runtime.recentEvents = []
        flash("已按时间清理 \(mutation.deletedEvents) 条统计事件")
        await refreshStatus(loadLatestEvents: true)
        refreshRuntimeMaintenance()
        return mutation
    }

    func deleteRuntimeSession(sessionID: String, confirmUnidentified: Bool = false) {
        Task {
            guard let admin else { return }
            do {
                let mutation = try await admin.deleteRuntimeSession(
                    sessionID: sessionID,
                    confirmUnidentified: confirmUnidentified
                )
                runtimeChangeSeq = 0
                runtimeEventDetails.removeAll()
                runtimeEventDetail = nil
                runtimePage = nil
                runHistoryRequestGeneration &+= 1
                runHistoryPage = nil
                runHistoryError = nil
                clearRuntimeV2Snapshot()
                runtimeV2RequestGeneration &+= 1
                runtimeErrorPageRequestGeneration &+= 1
                runtimeDimensionRequestGeneration &+= 1
                if runtimeLocalSessionID == sessionID || runtimeLocalSessionName == sessionID {
                    resetRuntimeLocalSessionState()
                }
                if runtimeAnalyticsSessionID == sessionID {
                    runtimeAnalyticsSessionID = ""
                }
                flash("已删除会话 · \(mutation.deletedRequests) 个请求 / \(mutation.deletedEvents) 条事件")
                await refreshStatus(loadLatestEvents: true)
            } catch {
                lastError = "\(error)"
                flash("删除会话失败")
            }
        }
    }

    /// 清除某项目的会话粘性归属。不删任何统计;只让该项目的粘性绑定失效,
    /// 下一个请求按入口库顺序重新选择入口。
    func clearProjectSticky(projectID: String) {
        Task {
            guard let admin else { return }
            do {
                let cleared = try await admin.clearProjectSticky(projectID: projectID)
                flash(cleared > 0 ? "已清除 \(cleared) 条粘性归属" : "该项目当前没有粘性归属")
            } catch {
                lastError = "\(error)"
                flash("清除粘性归属失败")
            }
        }
    }

    func exportRuntimeSession(sessionID: String) {
        Task {
            guard let admin else { return }
            do {
                let data = try await admin.exportRuntimeSession(sessionID: sessionID)
                let panel = NSSavePanel()
                panel.nameFieldStringValue = "sumpter-session-\(sessionID).json"
                if panel.runModal() == .OK, let url = panel.url {
                    try data.write(to: url, options: .atomic)
                    flash("会话 JSON 已导出")
                }
            } catch {
                lastError = "\(error)"
                flash("导出会话失败")
            }
        }
    }

}
