import SwiftUI

struct UnifiedAttributionPanel: View {
    @State private var client = "claude"
    @State private var shell = UnifiedAttributionInstaller.loginShell
    @State private var statuses: [UnifiedAttributionStatus] = []
    @State private var busy = false
    @State private var feedback: String?
    @State private var failed = false

    var body: some View {
        SectionPanel(title: "Claude / Grok / Gemini / Codex / pi 项目归因", hint: "检查并配置这台 Mac 上的客户端归因，需要 Node.js 18+；除 pi 外还需 bash/zsh。") {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    Picker("客户端", selection: $client) {
                        Text("Claude Code").tag("claude")
                        Text("Grok Build").tag("grok")
                        Text("Gemini CLI").tag("gemini")
                        Text("Codex CLI / TUI").tag("codex")
                        Text("pi").tag("pi")
                        Text("全部客户端").tag("all")
                    }
                    Picker("终端 Shell", selection: $shell) {
                        Text("zsh").tag("zsh")
                        Text("bash").tag("bash")
                        if !["zsh", "bash"].contains(shell) { Text(shell).tag(shell) }
                    }
                    .disabled(client == "pi")
                }
                .disabled(busy)
                if statuses.isEmpty {
                    Label(busy ? "正在检查本机配置…" : failed ? "无法确认当前安装状态" : "尚未检查本机配置",
                          systemImage: failed ? "exclamationmark.triangle" : "magnifyingglass")
                        .foregroundStyle(.secondary)
                }
                ForEach(statuses) { item in
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: item.status == "installed" ? "checkmark.circle.fill" : "info.circle")
                            .foregroundStyle(item.status == "installed" ? Color.green : Color.secondary)
                        VStack(alignment: .leading, spacing: 3) {
                            Text("\(clientTitle(item.client))：\(item.title)").font(.callout.weight(.semibold))
                            Text("\(item.client == "pi" ? "扩展文件" : item.shell) · \(item.rc)").font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                        }
                    }
                }
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 10) { buttons }
                    VStack(alignment: .leading, spacing: 10) { buttons }
                }
                if let feedback {
                    Text(feedback).font(.caption).foregroundStyle(failed ? Color.red : Color.green)
                        .textSelection(.enabled)
                }
                Text("安装前自动备份。还原仅恢复所选客户端的归因块或 pi 扩展；pi 原先未安装时会移除扩展。pi 操作后执行 /reload，其他客户端新开终端。")
                    .font(.caption).foregroundStyle(.secondary)
                DisclosureGroup("归因说明与前提") {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("按启动时的 Git 根目录（非 Git 目录使用当前目录）发送项目、工作区、用户和已脱敏的 Git remote。中文目录使用 URI 编码。")
                        Text("Codex 支持 -C / --cd，自动使用已有连接，无需指定 provider。新开终端后通过 codex 命令启动才生效；桌面 App 和已运行会话不会加载终端包装器。")
                        Text("请先将客户端连接到 Sumpter。Gemini 还需设置 SUMPTER_GEMINI_BASE_URL 和 SUMPTER_AUTH_TOKEN；密钥不写入归因配置。")
                        Text("pi 安装到 ~/.pi/agent/extensions/，不修改 shell 或 provider 配置。请在 ~/.pi/agent/models.json 的 Sumpter provider 中设置 headers: { \"X-Sumpter-Client\": \"pi\" }。扩展按当前项目和会话更新归因。")
                        Text("这里检查归因配置是否已安装。实际归因请在发送请求后查看统计；专用 header 不发往上游。")
                    }.font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .task(id: client + ":" + shell) { await execute(.status) }
    }

    @ViewBuilder private var buttons: some View {
        Button("检查状态") { run(.status) }
            .disabled(busy || UnifiedAttributionInstaller.scriptURL == nil)
        Button(statuses.contains { ["outdated", "legacy", "broken"].contains($0.status) } ? "安装 / 更新配置" : "安装配置") { run(.install) }
            .buttonStyle(.borderedProminent)
            .disabled(busy || statuses.isEmpty || UnifiedAttributionInstaller.scriptURL == nil)
        Button("还原配置") { run(.restore) }
            .disabled(busy || !statuses.contains(where: \.canRestore))
            .help("只还原所选客户端的归因配置；没有还原记录的客户端保持原样。")
        if busy { ProgressView().controlSize(.small) }
    }

    private func clientTitle(_ name: String) -> String {
        switch name {
        case "claude": "Claude Code"
        case "grok": "Grok Build"
        case "gemini": "Gemini CLI"
        case "codex": "Codex CLI / TUI"
        default: name
        }
    }

    private func run(_ action: ClaudeAttributionScriptAction) {
        Task { await execute(action) }
    }

    @MainActor private func execute(_ action: ClaudeAttributionScriptAction) async {
        guard !busy else { return }
        busy = true
        failed = false
        feedback = nil
        statuses = []
        defer { busy = false }
        do {
            let output = try await UnifiedAttributionInstaller.run(action, client: client, shell: shell)
            if action == .status {
                statuses = try UnifiedAttributionInstaller.parseStatus(output)
            } else {
                let nextStep = client == "pi" ? "请在 pi 中执行 /reload。"
                    : client == "all" ? "请新开终端；pi 请执行 /reload。" : "请新开终端。"
                feedback = (action == .install ? "配置已安装。" : "配置已还原。") + nextStep
                do {
                    let statusOutput = try await UnifiedAttributionInstaller.run(.status, client: client, shell: shell)
                    statuses = try UnifiedAttributionInstaller.parseStatus(statusOutput)
                } catch {
                    failed = true
                    feedback = "操作已完成，但状态复查失败：\(error.localizedDescription)"
                }
            }
        } catch {
            failed = true
            feedback = error.localizedDescription
        }
    }
}
