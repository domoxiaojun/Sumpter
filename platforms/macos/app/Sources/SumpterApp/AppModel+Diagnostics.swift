import AppKit
import Foundation
import ServiceManagement
import SwiftUI
import UserNotifications
import SumpterCore

@MainActor
extension AppModel {
    func refreshDiagnosticCapture() {
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureRequestGeneration &+= 1
        let generation = diagnosticCaptureRequestGeneration
        diagnosticCaptureBusy = true
        diagnosticCaptureTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureRequestGeneration {
                    self.diagnosticCaptureBusy = false
                    self.diagnosticCaptureTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "请先启动代理"
                return
            }
            do {
                let index = try await admin.diagnosticCaptureIndex()
                guard !Task.isCancelled, generation == self.diagnosticCaptureRequestGeneration else { return }
                if self.diagnosticCapture != index {
                    self.diagnosticCapture = index
                }
                self.diagnosticCaptureError = nil
                if let selected = self.diagnosticCaptureDetail?.requestID,
                   !index.records.contains(where: { $0.requestID == selected }) {
                    self.diagnosticCaptureDetailTask?.cancel()
                    self.diagnosticCaptureDetailRequestGeneration &+= 1
                    self.diagnosticCaptureDetail = nil
                    self.diagnosticCaptureDetailError = nil
                    self.diagnosticCaptureDetailBusy = false
                }
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "\(error)"
                self.flash("读取诊断捕获索引失败")
            }
        }
    }

    /// 只为用户当前选中的请求读取明文详情；快速切换时取消上一条并采用 latest-wins。
    func loadDiagnosticCaptureDetail(id: String?) {
        diagnosticCaptureDetailTask?.cancel()
        diagnosticCaptureDetailRequestGeneration &+= 1
        let generation = diagnosticCaptureDetailRequestGeneration
        guard let id, !id.isEmpty else {
            diagnosticCaptureDetail = nil
            diagnosticCaptureDetailError = nil
            diagnosticCaptureDetailBusy = false
            return
        }
        diagnosticCaptureDetail = nil
        diagnosticCaptureDetailError = nil
        diagnosticCaptureDetailBusy = true
        diagnosticCaptureDetailTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureDetailRequestGeneration {
                    self.diagnosticCaptureDetailBusy = false
                    self.diagnosticCaptureDetailTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureDetailRequestGeneration else { return }
                self.diagnosticCaptureDetailError = "请先启动代理"
                return
            }
            do {
                // 详情正文可能接近捕获容量上限。显式把网络读取和 Codable 解码
                // 放到 utility executor，只有最终的小状态更新回主 actor，避免把
                // 大记录解析与详情页的首帧布局耦合在一起。
                let detailTask = Task.detached(priority: .utility) {
                    try await admin.diagnosticCaptureDetail(id: id)
                }
                let detail = try await withTaskCancellationHandler(operation: {
                    try await detailTask.value
                }, onCancel: {
                    detailTask.cancel()
                })
                guard !Task.isCancelled, generation == self.diagnosticCaptureDetailRequestGeneration else { return }
                self.diagnosticCaptureDetail = detail
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureDetailRequestGeneration else { return }
                self.diagnosticCaptureDetailError = "\(error)"
                self.flash("读取诊断捕获详情失败")
            }
        }
    }

    /// 将最近一次原子落盘的完整捕获快照直接下载到用户选择的文件，不把正文解码或
    /// 累积进 AppModel。URLSession download task 会在临时文件中接收流，适合接近
    /// 512 MiB 的捕获；导出文件包含所有记录（索引最多只展示最近 200 条）。
    func exportDiagnosticCapture(
        privacy: String = "raw",
        confirmRaw: Bool = false,
        format: String = "jsonl",
        scope: String = "all",
        requestID: String? = nil
    ) {
        guard !diagnosticCaptureExportBusy else { return }
        guard let admin, (diagnosticCapture?.recordCount ?? 0) > 0 else {
            flash("暂无可导出的诊断捕获")
            return
        }
        guard privacy == "redacted" || (privacy == "raw" && confirmRaw) else {
            flash("未确认原始诊断捕获导出")
            return
        }
        let panel = NSSavePanel()
        let extensionName = format == "json" ? "json" : "jsonl"
        panel.nameFieldStringValue = "sumpter-diagnostic-capture-\(privacy)-\(Int(Date().timeIntervalSince1970)).\(extensionName)"
        panel.message = privacy == "raw"
            ? "文件包含未脱敏请求、响应、Headers 和流式 Chunk；仅保存到可信位置。"
            : "服务端会按需生成脱敏快照；原始捕获文件不会加载到 App 内存。"
        guard panel.runModal() == .OK, let destination = panel.url else { return }

        diagnosticCaptureExportRequestGeneration &+= 1
        let generation = diagnosticCaptureExportRequestGeneration
        diagnosticCaptureExportBusy = true
        let task = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureExportRequestGeneration {
                    self.diagnosticCaptureExportTask = nil
                    self.diagnosticCaptureExportBusy = false
                }
            }
            do {
                let downloadTask = Task.detached(priority: .utility) {
                    try await admin.downloadDiagnosticCapture(
                        to: destination,
                        privacy: privacy,
                        confirmRaw: confirmRaw,
                        scope: scope,
                        format: format,
                        requestID: requestID
                    )
                }
                try await withTaskCancellationHandler(operation: {
                    try await downloadTask.value
                }, onCancel: {
                    downloadTask.cancel()
                })
                guard !Task.isCancelled else { return }
                self.flash(privacy == "raw" ? "原始诊断捕获已导出" : "脱敏诊断捕获已导出")
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                self.flash("完整诊断捕获导出失败：\(error.localizedDescription)")
            }
        }
        diagnosticCaptureExportTask = task
    }

    func setDiagnosticCapture(enabled: Bool, maxBytes: Int? = nil) {
        guard !diagnosticCaptureBusy else { return }
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureRequestGeneration &+= 1
        let generation = diagnosticCaptureRequestGeneration
        diagnosticCaptureBusy = true
        diagnosticCaptureTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureRequestGeneration {
                    self.diagnosticCaptureBusy = false
                    self.diagnosticCaptureTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "请先启动代理"
                return
            }
            do {
                let index = try await admin.setDiagnosticCapture(enabled: enabled, maxBytes: maxBytes)
                guard !Task.isCancelled, generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCapture = index
                self.diagnosticCaptureError = nil
                self.flash(enabled ? "完整诊断捕获已开始" : "完整诊断捕获已停止")
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "\(error)"
                self.flash("切换诊断捕获失败")
            }
        }
    }

    func clearDiagnosticCapture() {
        guard !diagnosticCaptureBusy else { return }
        diagnosticCaptureTask?.cancel()
        diagnosticCaptureRequestGeneration &+= 1
        let generation = diagnosticCaptureRequestGeneration
        diagnosticCaptureBusy = true
        diagnosticCaptureDetailTask?.cancel()
        diagnosticCaptureDetailRequestGeneration &+= 1
        diagnosticCaptureDetail = nil
        diagnosticCaptureDetailError = nil
        diagnosticCaptureDetailBusy = false
        diagnosticCaptureTask = Task { [weak self] in
            guard let self else { return }
            defer {
                if generation == self.diagnosticCaptureRequestGeneration {
                    self.diagnosticCaptureBusy = false
                    self.diagnosticCaptureTask = nil
                }
            }
            guard let admin = self.admin else {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "请先启动代理"
                return
            }
            do {
                try await admin.clearDiagnosticCapture()
                let index = try await admin.diagnosticCaptureIndex()
                guard !Task.isCancelled, generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCapture = index
                self.diagnosticCaptureError = nil
                self.flash("捕获记录已清空")
            } catch is CancellationError {
                return
            } catch {
                guard generation == self.diagnosticCaptureRequestGeneration else { return }
                self.diagnosticCaptureError = "\(error)"
                self.flash("清空诊断捕获失败")
            }
        }
    }

    /// 同步等待刷新完成;冻结模式的手动刷新用它拿到最新快照再重新冻结。
}
