import SwiftUI

enum HelpOnboardingState: String, Equatable {
    case notStarted = "not_started"
    case notConfigured = "not_configured"
    case noMapping = "no_mapping"
    case clientNotConnected = "client_not_connected"
    case firstFailure = "first_failure"
    case firstSuccess = "first_success"
}

/// Keep the state transition independent from SwiftUI and AppModel so the
/// same six-state contract can be exercised without starting a sidecar.
func resolveHelpOnboardingState(
    isRunning: Bool,
    hasConfig: Bool,
    hasMapping: Bool,
    clientRequests: Int,
    clientSuccesses: Int,
    clientFailures: Int
) -> HelpOnboardingState {
    guard isRunning else { return .notStarted }
    guard hasConfig else { return .notConfigured }
    guard hasMapping else { return .noMapping }
    guard clientRequests > 0 else { return .clientNotConnected }
    if clientSuccesses > 0 { return .firstSuccess }
    if clientFailures > 0 { return .firstFailure }
    return .clientNotConnected
}

struct HelpPane: View {
    @ObservedObject var model: AppModel
    var onNavigate: (SettingsSection) -> Void = { _ in }

    private var piExtensionPath: String? {
        (Bundle.main.url(forResource: "pi-project-attribution", withExtension: "ts")
            ?? Bundle.module.url(forResource: "pi-project-attribution", withExtension: "ts"))?.path
    }

    private var geminiWrapperPath: String? {
        (Bundle.main.url(forResource: "gemini-sumpter-wrapper", withExtension: "mjs")
            ?? Bundle.module.url(forResource: "gemini-sumpter-wrapper", withExtension: "mjs"))?.path
    }

    private var onboardingState: HelpOnboardingState {
        let counters = model.runtimeSummary?.counters
        return resolveHelpOnboardingState(
            isRunning: model.sidecarState == .running && model.isProxyRunning,
            hasConfig: !model.configPath.isEmpty && !model.config.endpoints.isEmpty,
            hasMapping: model.config.hasRoutableModel,
            clientRequests: counters?.clientRequests ?? model.runtime.clientRequests,
            clientSuccesses: counters?.clientSuccesses ?? model.runtime.clientSuccesses,
            clientFailures: counters?.clientFailures ?? model.runtime.clientFailures
        )
    }

    private var onboardingTitle: String {
        switch onboardingState {
        case .notStarted: "未启动"
        case .notConfigured: "未配置"
        case .noMapping: "无可用模型"
        case .clientNotConnected: "客户端未接入"
        case .firstFailure: "首次失败"
        case .firstSuccess: "首次成功"
        }
    }

    private var onboardingDetail: String {
        switch onboardingState {
        case .notStarted: "代理还没有进入运行态。先启动 sidecar，再继续配置入口。"
        case .notConfigured: "入口库尚未配置连接。先添加一个入口并保存。"
        case .noMapping: "入口已配置，但没有启用的模型与入口绑定。请在模型组中选择模型并绑定入口。"
        case .clientNotConnected: "还没有观察到客户端请求。把 Claude Code 或 Codex 的 Base URL 指向当前代理。"
        case .firstFailure: "已收到客户端请求，但还没有成功请求。先查看请求链中的失败阶段和上游响应。"
        case .firstSuccess: "已完成至少一次客户端成功请求。可以继续查看请求链和 failover 结果。"
        }
    }

    private var onboardingAction: String {
        switch onboardingState {
        case .notStarted: "前往运行并启动"
        case .notConfigured: "前往入口库添加连接"
        case .noMapping: "前往模型组配置模型"
        case .clientNotConnected: "查看客户端接入说明"
        case .firstFailure: "前往运行查看请求链"
        case .firstSuccess: "查看成功请求链"
        }
    }

    var body: some View {
        SettingsPage(title: SettingsSection.help.title, subtitle: SettingsSection.help.subtitle) {
            SectionPanel(title: "当前开箱状态", hint: "只使用运行、配置和运行统计快照；不会展示密钥或诊断原文。") {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .firstTextBaseline) {
                        Text(onboardingTitle).font(.headline)
                        Spacer(minLength: 0)
                        Text(onboardingState.rawValue)
                            .font(.caption.monospaced())
                            .foregroundStyle(.secondary)
                    }
                    Text(onboardingDetail)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    if onboardingState == .clientNotConnected {
                        Link(destination: URL(string: "https://github.com/domoxiaojun/sumpter/blob/main/USAGE.md")!) {
                            Label(onboardingAction, systemImage: "book")
                        }
                    } else {
                        Button(onboardingAction) {
                            let destination: SettingsSection = switch onboardingState {
                            case .notStarted, .firstFailure, .firstSuccess: .run
                            case .notConfigured: .providers
                            case .noMapping: .modelGroups
                            case .clientNotConnected: .help
                            }
                            onNavigate(destination)
                        }
                        .buttonStyle(.borderedProminent)
                    }
                }
            }
            SectionPanel(title: "快速开始", hint: "Sumpter是本机协议代理；先启动代理，再把客户端 API Base 指向监听地址。") {
                VStack(alignment: .leading, spacing: 10) {
                    HelpStep(number: 1, title: "准备上游服务入口", bodyText: "在“入口库”添加地址和密钥，再到“模型组”选择模型并绑定入口；启用组和入口后保存。")
                    HelpStep(number: 2, title: "启动并确认监听", bodyText: "回到“运行”页确认 sidecar 正在运行。默认代理地址是 http://127.0.0.1:57878；实际地址以运行页显示为准。")
                    HelpStep(number: 3, title: "连接客户端", bodyText: "Claude Code 使用 ANTHROPIC_BASE_URL；Codex 或其它 OpenAI 客户端必须使用带 /v1 的 API Base（默认 http://127.0.0.1:57878/v1）。具体变量和协议矩阵见 USAGE.md。")
                }
            }

            SectionPanel(title: "pi 客户端与项目归因", hint: "在运行 pi 的主机配置，发送请求后在运行与统计页验证。") {
                VStack(alignment: .leading, spacing: 10) {
                    Text("编辑 ~/.pi/agent/models.json，配置 Sumpter provider 的 API、Base URL、入站 Token 与已启用的客户端模型名。OpenAI 使用 /v1，Anthropic 使用根地址，Gemini 使用 /v1beta。")
                        .font(.callout).foregroundStyle(.secondary)
                    InfoRow(title: "provider 标识", value: #""headers": { "X-Sumpter-Client": "pi" }"#, copyable: true)
                    InfoRow(title: "Token 环境变量", value: #""apiKey": "$SUMPTER_API_KEY""#, copyable: true)
                    if let path = piExtensionPath {
                        let quoted = "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
                        InfoRow(title: "临时加载", value: "pi -e " + quoted, copyable: true)
                        InfoRow(title: "安装扩展", value: "mkdir -p \"$HOME/.pi/agent/extensions\" && cp " + quoted + " \"$HOME/.pi/agent/extensions/pi-project-attribution.ts\"", copyable: true)
                    } else {
                        Text("App 缺少 pi 扩展资源，请使用完整安装包或仓库 scripts/pi-project-attribution.ts。")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Text("已有扩展先备份再更新；安装后执行 /reload。扩展仅为显式标记的 Sumpter provider 添加当前项目和会话。删除 ~/.pi/agent/extensions/pi-project-attribution.ts 并重新加载即可移除。")
                        .font(.caption).foregroundStyle(.secondary)
                    Link("完整 pi 配置与安装说明", destination: URL(string: "https://github.com/domoxiaojun/sumpter/blob/main/USAGE.md#pi-客户端")!)
                }
            }

            UnifiedAttributionPanel()

            SectionPanel(title: "配置与接入", hint: "两端均使用当前 schema 的 config.json；旧版本迁移会先创建备份。") {
                VStack(alignment: .leading, spacing: 10) {
                    Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 8) {
                        InfoRow(title: "macOS 配置路径", value: model.configPath.isEmpty ? "~/Library/Application Support/Sumpter/config.json" : model.configPath, copyable: !model.configPath.isEmpty)
                        InfoRow(title: "Claude Code", value: "ANTHROPIC_BASE_URL=http://127.0.0.1:57878")
                        InfoRow(title: "Codex / OpenAI", value: "API Base=http://127.0.0.1:57878/v1")
                    }
                    Text("不要把 config.json、API key、入站 Token 或诊断原文提交到 Git，也不要粘贴到公开 Issue。")
                        .font(.callout)
                        .foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            SectionPanel(title: "常见问题", hint: "先看运行页和诊断页的状态，再判断是客户端、代理还是上游问题。") {
                VStack(alignment: .leading, spacing: 10) {
                    HelpFAQ(question: "Claude Code 连不上？", answer: "确认 Base URL 指向当前监听地址；若启用了入站认证，客户端 Token 必须与配置完全一致。")
                    HelpFAQ(question: "请求返回模型未找到？", answer: "检查启用模型组是否声明客户端模型名，组内是否绑定了启用入口；旧配置也可检查入口 mappings。")
                    HelpFAQ(question: "Realtime、Files 或 Videos 失败？", answer: "这些能力由代理直接 relay 给上游：先检查 Provider 的 baseURL、API key、模型 mapping，以及上游是否开放对应 HTTP/WebSocket 能力。代理不会在本地重建协议。")
                }
            }

            SectionPanel(title: "文档与反馈") {
                SumpterWrappingLayout(horizontalSpacing: 14, verticalSpacing: 10) {
                    Link(destination: URL(string: "https://github.com/domoxiaojun/sumpter")!) {
                        Label("GitHub 仓库", systemImage: "chevron.left.forwardslash.chevron.right")
                    }
                    Link(destination: URL(string: "https://github.com/domoxiaojun/sumpter/blob/main/USAGE.md")!) {
                        Label("USAGE.md", systemImage: "book")
                    }
                    Link(destination: URL(string: "https://github.com/domoxiaojun/sumpter/issues")!) {
                        Label("提交 Issue", systemImage: "exclamationmark.bubble")
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text("仓库根目录的 USAGE.md 是开箱、配置和 Claude Code / Codex 接入的完整指南。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

struct AboutPane: View {
    @ObservedObject var model: AppModel

    private var appVersion: String {
        if let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
           !version.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return version
        }
        return "开发构建"
    }

    var body: some View {
        SettingsPage(title: SettingsSection.about.title, subtitle: SettingsSection.about.subtitle, maxWidth: 760) {
            SectionPanel(title: "Sumpter · Sumpter", hint: "本地多上游 AI 协议代理") {
                VStack(alignment: .leading, spacing: 12) {
                    Text("支持 Claude Code / Codex 的协议适配、智能路由、故障转移与运行统计。")
                        .fixedSize(horizontal: false, vertical: true)
                    Divider()
                    AboutRow(title: "作者 / Maintainer", value: "Domo Mido")
                    AboutRow(title: "版本", value: appVersion)
                    AboutRow(title: "许可证", value: "MIT License")
                    AboutRow(title: "配置", value: model.configPath.isEmpty ? "尚未读取" : model.configPath)
                }
            }

            SectionPanel(title: "项目链接") {
                VStack(alignment: .leading, spacing: 10) {
                    Link(destination: URL(string: "https://github.com/domoxiaojun/sumpter")!) {
                        Label {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Sumpter GitHub 仓库")
                                Text("源代码、Issue 与版本发布")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        } icon: {
                            Image(systemName: "chevron.left.forwardslash.chevron.right")
                        }
                    }
                    Link(destination: URL(string: "https://github.com/domoxiaojun/sumpter/issues")!) {
                        Label {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("反馈问题或提出建议")
                                Text("在 GitHub Issue 中提交反馈")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        } icon: {
                            Image(systemName: "exclamationmark.bubble")
                        }
                    }
                }
            }

            Text("本项目与 Anthropic、OpenAI 及其产品无隶属关系。")
                .font(.caption)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
    }
}

private struct HelpStep: View {
    let number: Int
    let title: String
    let bodyText: String

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(number)")
                .font(.caption.weight(.bold))
                .frame(width: 22, height: 22)
                .background(Color.accentColor.opacity(0.15), in: Circle())
                .foregroundStyle(Color.accentColor)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.subheadline.weight(.semibold))
                Text(bodyText).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

private struct HelpFAQ: View {
    let question: String
    let answer: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(question).font(.subheadline.weight(.semibold))
            Text(answer).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct AboutRow: View {
    let title: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 16) {
            Text(title).foregroundStyle(.secondary).frame(width: 150, alignment: .leading)
            Text(value).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
    }
}
