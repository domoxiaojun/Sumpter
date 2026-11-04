import SumpterCore
import SwiftUI

struct UnifiedAttributionPanel: View {
    @ObservedObject var model: AppModel
    @State private var client = "claude"
    @State private var shell = UnifiedAttributionInstaller.loginShell
    @State private var proxyURL = ""
    @State private var statuses: [UnifiedAttributionStatus] = []
    @State private var busy = false
    @State private var feedback: String?
    @State private var failed = false
    @State private var confirmRestore = false

    var body: some View {
        SectionPanel(title: "客户端项目归因", hint: "在运行客户端的这台 Mac 上启用，需要 Node.js 18+ 和 bash/zsh。") {
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
                }.disabled(busy)
                TextField("绑定的 Sumpter 地址", text: $proxyURL)
                    .textFieldStyle(.roundedBorder).disabled(busy)
                Text("填写客户端实际连接的 Sumpter 地址。地址不匹配或无法确认时跳过补充采集；pi 以 provider 的显式标记为准。")
                    .font(.caption).foregroundStyle(.secondary)
                if statuses.isEmpty {
                    Text(busy ? "正在检查本机配置…" : "尚未确认本机配置")
                        .font(.callout).foregroundStyle(.secondary)
                }
                ForEach(statuses) { item in statusRow(item) }
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 10) { buttons }
                    VStack(alignment: .leading, spacing: 10) { buttons }
                }
                if let feedback {
                    Text(feedback).font(.callout).foregroundStyle(failed ? Color.red : Color.primary)
                        .textSelection(.enabled)
                }
                Text("首次启用后新开终端并启动客户端。关闭后请重启已运行的 Claude/Grok/Gemini/Codex；新版 pi 在下一次请求读取开关。关闭只停止脚本补充，不删除历史或原生客户端信息。")
                    .font(.caption).foregroundStyle(.secondary)
                DisclosureGroup("高级操作与使用条件") {
                    VStack(alignment: .leading, spacing: 12) {
                        Button("更新脚本，保留开关状态") { run(.install) }
                            .frame(minHeight: 44).disabled(busy || statuses.isEmpty)
                        Button("恢复安装前配置…") { confirmRestore = true }
                            .frame(minHeight: 44).disabled(busy || !statuses.contains(where: \.canRestore))
                        Button("移除终端注册") { run(.uninstall) }
                            .frame(minHeight: 44).disabled(busy || statuses.isEmpty)
                        Text("恢复备份可能恢复旧版归因；想停止补充采集，请使用“关闭归因”。移除终端注册后 Gemini 不再由包装器准备连接，请自行保留客户端连接配置。")
                        Text("备份：" + statuses.map { "\(clientTitle($0.client)) · \($0.backupState == "invalid" ? "记录损坏，仍可关闭" : $0.canRestore ? ($0.restoreTarget == "legacy" ? "将恢复旧版配置" : "将移除归因注册") : "无待恢复配置")" }.joined(separator: "；"))
                        Text("Gemini 需设置 SUMPTER_GEMINI_BASE_URL 和 SUMPTER_AUTH_TOKEN；关闭归因保留其 URL 与认证准备。密钥不写入归因配置。pi 需在对应 provider 添加 x-sumpter-client: pi，包装器自动加载配套扩展。")
                        Text("Codex 使用已有自定义 provider，支持 -C / --cd。复杂项目/CLI 配置无法确认时跳过补充；桌面 App 不加载终端包装器。Grok 需确认 GROK_CONFIG 的两个请求端点均指向绑定地址。")
                        Text("脚本发送启动时的完整工作区路径、项目、用户和去除凭据的 Git remote 到 Sumpter。服务端先生成项目身份，再脱敏显示；私有头不发往上游。启动记录仅代表包装器准备结果。")
                    }.font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .task(id: client + ":" + shell) { await execute(.status) }
        .confirmationDialog("恢复安装前配置？", isPresented: $confirmRestore) {
            Button("恢复安装前配置", role: .destructive) { run(.restore) }
            Button("取消", role: .cancel) {}
        } message: {
            Text(statuses.contains { $0.canRestore && $0.restoreTarget == "legacy" }
                 ? "所选备份包含旧版归因配置，恢复后可能继续采集。若要关闭，请取消并使用“关闭归因”。"
                 : "将恢复所选客户端安装前的状态；保留其他终端配置与备份。")
        }
    }

    @ViewBuilder private func statusRow(_ item: UnifiedAttributionStatus) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("\(clientTitle(item.client))：\(item.title)").font(.callout.weight(.semibold))
            Text("开关：\(item.enabled == true ? "启用" : "关闭") · 绑定：\(item.proxyURL ?? "未配置")")
            Text("脚本：\(item.installedVersion ?? "未安装") · 可用：\(item.availableVersion ?? "未知")")
            if let launch = item.lastLaunch {
                Text("启动记录：\(launch.startedAt) · \(bindingTitle(launch.binding)) · \(launch.enabled ? "已准备采集" : "已跳过采集")")
            } else { Text("启动记录：尚未记录") }
            if let event = model.runtime.recentEvents.filter({ item.observes($0) }).max(by: { $0.timestamp < $1.timestamp }) {
                Text("请求证据：同一来源与脚本版本，\(event.timestamp.formatted(date: .abbreviated, time: .standard)) 已收到")
            } else { Text("请求证据：未在已加载请求中观察到同一来源与版本") }
            Text("\(item.shell) · \(item.rc)")
            if let note = item.note { Text(note).foregroundStyle(.orange) }
        }.font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
    }

    @ViewBuilder private var buttons: some View {
        Button("启用 / 更新归因") { run(.enable) }
            .buttonStyle(.borderedProminent).frame(minHeight: 44)
            .disabled(busy || statuses.isEmpty || proxyURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        Button("关闭归因") { run(.disable) }
            .frame(minHeight: 44).disabled(busy || !statuses.contains { $0.canDisable == true })
        Button("刷新状态") { run(.status) }.frame(minHeight: 44).disabled(busy)
        if busy { ProgressView().controlSize(.small) }
    }

    private func bindingTitle(_ value: String) -> String {
        switch value {
        case "matched": "连接匹配"
        case "mismatch": "连接不匹配"
        case "provider_opt_in": "等待 provider 显式标记"
        default: "连接无法确认"
        }
    }

    private func clientTitle(_ name: String) -> String {
        ["claude": "Claude Code", "grok": "Grok Build", "gemini": "Gemini CLI", "codex": "Codex CLI / TUI"][name] ?? name
    }

    private func run(_ action: UnifiedAttributionAction) { Task { await execute(action) } }

    @MainActor private func execute(_ action: UnifiedAttributionAction) async {
        guard !busy else { return }
        busy = true
        failed = false
        feedback = nil
        defer { busy = false }
        do {
            let output = try await UnifiedAttributionInstaller.run(action, client: client, shell: shell,
                proxyURL: action == .enable || action == .install ? proxyURL.trimmingCharacters(in: .whitespacesAndNewlines) : nil)
            statuses = try UnifiedAttributionInstaller.parseStatus(output)
            if action == .status {
                let host = ["0.0.0.0", "::"].contains(model.config.listener.host) ? "127.0.0.1" : model.config.listener.host
                let address = host.contains(":") ? "[\(host)]" : host
                proxyURL = statuses.first(where: { $0.installedVersion != nil })?.proxyURL ?? "http://\(address):\(model.config.listener.port)"
            } else {
                switch action {
                case .enable: feedback = "归因已启用。请新开终端并重启客户端，再发送请求验证。"
                case .disable: feedback = "补充采集已关闭。请重启已运行的客户端；新版 pi 在下一次请求生效。"
                case .install: feedback = "脚本已更新，开关状态保持原样。"
                case .restore: feedback = "已恢复安装前配置。请查看当前状态，旧版配置可能继续采集。"
                case .uninstall: feedback = "已移除终端注册并关闭新版补充采集，备份保留。请重启客户端。"
                case .status: break
                }
            }
        } catch {
            failed = true
            feedback = error.localizedDescription
        }
    }
}
