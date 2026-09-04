import AppKit
import SumpterCore
import SwiftUI

/// 本机 shell wrapper 安装面板。Claude Code 与 Grok Build 共用同一套 status/install/restore。
struct ProjectAttributionInstallerPanel: View {
    let title: String
    let subtitle: String
    let scriptResource: String
    let clientName: String
    let observedState: ClaudeAttributionHint.State
    let whereToRun: String
    let privacy: String
    let steps: [ClaudeAttributionHint.Guide.Step]
    let rollback: [ClaudeAttributionHint.Guide.RollbackCommand]
    let installSuccessMessage: String
    let missingScriptMessage: String
    let statusDetail: (ClaudeAttributionHint.State) -> String

    @State private var localStatus: ClaudeAttributionInstallationStatus?
    @State private var busyAction: ClaudeAttributionScriptAction?
    @State private var feedback: (message: String, succeeded: Bool)?
    @State private var lastOutput: String?
    @State private var advancedExpanded = false
    @State private var confirmRestore = false
    @State private var confirmUninstall = false

    var body: some View {
        SectionPanel(title: title, hint: subtitle) {
            VStack(alignment: .leading, spacing: 14) {
                statusRow
                actions
                if let feedback {
                    Text(feedback.message)
                        .font(.caption)
                        .foregroundStyle(feedback.succeeded ? Color.green : Color.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("配置后，新启动的 \(clientName) 会自动携带项目名；不会读取或修改提示词和请求正文。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                callout(whereToRun)
                Divider()
                observedStateRow
                advanced
            }
        }
        .task(id: scriptURL?.path) {
            await execute(.status, announceSuccess: false)
        }
        .confirmationDialog("恢复最近备份？", isPresented: $confirmRestore) {
            Button("恢复备份", role: .destructive) { run(.restore) }
            Button("取消", role: .cancel) {}
        }
        .confirmationDialog("移除 \(clientName) 项目归因配置？", isPresented: $confirmUninstall) {
            Button("移除配置", role: .destructive) { run(.uninstall) }
            Button("取消", role: .cancel) {}
        }
    }

    private var scriptURL: URL? {
        ClaudeAttributionInstaller.locateScript(named: scriptResource)
    }

    private var statusRow: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: localIcon)
                .font(.title3)
                .foregroundStyle(localColor)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 3) {
                Text(localTitle)
                    .font(.callout.weight(.semibold))
                Text(localDetail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            if busyAction != nil {
                ProgressView().controlSize(.small)
            }
        }
    }

    private var localTitle: String {
        guard scriptURL != nil else { return "配置器不可用" }
        guard let status = localStatus else {
            return busyAction == .status ? "正在检查本机配置…" : "尚未检查本机配置"
        }
        switch status.condition {
        case .installed: return "本机配置已安装"
        case .notInstalled: return "本机尚未配置"
        case .needsRepair: return "本机配置需要修复"
        case .blockedBySettings: return "发现配置冲突"
        }
    }

    private var localDetail: String {
        guard scriptURL != nil else { return missingScriptMessage }
        guard let status = localStatus else {
            return "检查当前登录 shell 和对应配置文件，不读取请求内容。"
        }
        switch status.condition {
        case .installed:
            return "已接入 \(status.shell) · \(status.rcPath)。新开终端后生效。"
        case .notInstalled:
            return "将配置 \(status.shell) · \(status.rcPath)，操作前会自动创建时间戳备份。"
        case .needsRepair:
            return "shell 标记仍在，但配套脚本缺失；可一键重新生成。"
        case .blockedBySettings:
            return "环境里有会挡住 wrapper 的配置，请先在高级说明中查看处理方法。"
        }
    }

    private var localIcon: String {
        guard scriptURL != nil else { return "xmark.octagon.fill" }
        guard let status = localStatus else { return "questionmark.circle" }
        switch status.condition {
        case .installed: return "checkmark.circle.fill"
        case .notInstalled: return "circle.dashed"
        case .needsRepair: return "wrench.and.screwdriver.fill"
        case .blockedBySettings: return "exclamationmark.triangle.fill"
        }
    }

    private var localColor: Color {
        guard scriptURL != nil else { return .red }
        guard let status = localStatus else { return .secondary }
        switch status.condition {
        case .installed: return .green
        case .notInstalled: return .secondary
        case .needsRepair, .blockedBySettings: return .orange
        }
    }

    private var primaryAction: ClaudeAttributionScriptAction {
        switch localStatus?.condition {
        case .installed: .status
        case .notInstalled, .needsRepair, nil: .install
        case .blockedBySettings: .status
        }
    }

    private var primaryTitle: String {
        guard scriptURL != nil else { return "配置器不可用" }
        switch localStatus?.condition {
        case .installed: return "检查配置"
        case .needsRepair: return "一键修复配置"
        case .blockedBySettings: return "需先处理冲突"
        case .notInstalled, nil: return "一键配置本机"
        }
    }

    @ViewBuilder
    private var actions: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) { actionButtons }
            VStack(alignment: .leading, spacing: 8) { actionButtons }
        }
    }

    @ViewBuilder
    private var actionButtons: some View {
        Button {
            run(primaryAction)
        } label: {
            Label(primaryTitle, systemImage: busyAction == primaryAction ? "hourglass" : "hammer")
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.large)
        .disabled(busyAction != nil || scriptURL == nil || localStatus?.condition == .blockedBySettings)

        if localStatus?.canRestore == true {
            Button("恢复备份…") { confirmRestore = true }
                .controlSize(.large)
                .disabled(busyAction != nil)
        }
        if localStatus?.canRemove == true {
            Button("移除配置…", role: .destructive) { confirmUninstall = true }
                .controlSize(.large)
                .disabled(busyAction != nil)
        }
    }

    private var observedStateRow: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("请求验证：\(ClaudeAttributionHint.Guide.statusLabel(observedState))")
                .font(.caption.weight(.semibold))
            Text(statusDetail(observedState))
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var advanced: some View {
        DisclosureGroup(isExpanded: $advancedExpanded) {
            VStack(alignment: .leading, spacing: 12) {
                Text(privacy)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(steps) { step in
                    VStack(alignment: .leading, spacing: 4) {
                        Text("\(step.id). \(step.title)")
                            .font(.caption.weight(.semibold))
                        if !step.command.isEmpty {
                            commandRow(step.command)
                        }
                        Text(step.note)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                ForEach(rollback) { item in
                    commandRow(item.command)
                    Text(item.note)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let output = lastOutput, !output.isEmpty {
                    Text(output)
                        .font(.caption2.monospaced())
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.top, 8)
        } label: {
            Text("高级说明与命令")
                .font(.caption.weight(.semibold))
        }
    }

    private func callout(_ text: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "desktopcomputer.and.arrow.down")
                .foregroundStyle(.orange)
            Text(text)
                .font(.caption)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(10)
        .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
    }

    private func commandRow(_ command: String) -> some View {
        HStack(spacing: 8) {
            Text(command)
                .font(.caption2.monospaced())
                .textSelection(.enabled)
            Button("复制") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(command, forType: .string)
            }
            .controlSize(.small)
        }
    }

    private func run(_ action: ClaudeAttributionScriptAction) {
        guard busyAction == nil else { return }
        Task { await execute(action, announceSuccess: true) }
    }

    @MainActor
    private func execute(_ action: ClaudeAttributionScriptAction, announceSuccess: Bool) async {
        guard busyAction == nil else { return }
        guard let scriptURL else {
            feedback = ("找不到项目归因配置器，请重新安装完整 App。", false)
            return
        }
        busyAction = action
        if action != .status { feedback = nil }
        defer { busyAction = nil }
        do {
            let execution = try await ClaudeAttributionInstaller.run(scriptURL: scriptURL, action: action)
            lastOutput = execution.combinedOutput
            guard execution.succeeded else {
                let detail = execution.combinedOutput.split(whereSeparator: \.isNewline).first.map(String.init)
                    ?? "配置器未返回错误详情"
                feedback = ("操作失败（退出码 \(execution.exitCode)）：\(detail)", false)
                if action == .status { localStatus = nil }
                return
            }
            if action == .status {
                localStatus = try ClaudeAttributionInstaller.parseStatus(execution.standardOutput)
                if announceSuccess { feedback = ("本机配置检查完成。", true) }
                return
            }
            switch action {
            case .install: feedback = (installSuccessMessage, true)
            case .restore: feedback = ("已恢复最近备份。请新开终端窗口使其生效。", true)
            case .uninstall: feedback = ("项目归因配置已移除，已有备份仍保留。", true)
            case .status: break
            }
            let refreshed = try await ClaudeAttributionInstaller.run(scriptURL: scriptURL, action: .status)
            if refreshed.succeeded {
                localStatus = try ClaudeAttributionInstaller.parseStatus(refreshed.standardOutput)
            }
        } catch {
            feedback = (error.localizedDescription, false)
            if action == .status { localStatus = nil }
        }
    }
}
