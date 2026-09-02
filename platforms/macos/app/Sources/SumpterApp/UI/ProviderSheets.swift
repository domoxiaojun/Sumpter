import SumpterCore
import SwiftUI

struct ProviderAccountEditorSheet: View {
    @ObservedObject var model: AppModel
    let providerName: String
    let row: EndpointDisplayRow?
    let onClose: () -> Void

    @State private var idText: String
    @State private var name: String
    @State private var baseURL: String
    @State private var protocolName: String
    @State private var enabled: Bool
    @State private var apiKey: String
    @State private var pinnedIPs: String
    @State private var priorityText: String
    @State private var pin: Bool
    @State private var stickyGroup: String
    @State private var keepAlive: Bool
    @State private var submission = SubmissionState.idle
    @State private var showPricingEditor = false

    init(
        model: AppModel,
        providerName: String,
        row: EndpointDisplayRow?,
        onClose: @escaping () -> Void
    ) {
        self.model = model
        self.providerName = providerName
        self.row = row
        self.onClose = onClose
        _idText = State(initialValue: row?.id ?? "")
        _name = State(initialValue: row?.name ?? "")
        _baseURL = State(initialValue: row?.baseURL ?? "https://")
        _protocolName = State(initialValue: row?.protocolName ?? EndpointProtocolMode.auto.rawValue)
        _enabled = State(initialValue: row?.enabled ?? true)
        _apiKey = State(initialValue: row?.apiKey ?? "")
        _pinnedIPs = State(initialValue: row?.pinnedIPsText ?? "")
        _priorityText = State(initialValue: row?.priorityText ?? "0")
        _pin = State(initialValue: row?.pinnedIPExclusive ?? false)
        _stickyGroup = State(initialValue: row?.stickyGroup ?? "")
        // 新入口默认开启；编辑已有入口时保留磁盘中的显式值。
        _keepAlive = State(initialValue: row?.keepAlive ?? true)
    }

    var body: some View {
        SheetShell(
            title: row == nil ? "添加 Provider" : "编辑 Provider",
            primaryTitle: submission.isSubmitting ? "保存中..." : (row == nil ? "添加" : "保存"),
            primaryDisabled: submission.isSubmitting || !canSubmit,
            onCancel: onClose,
            onSubmit: submit
        ) {
            VStack(alignment: .leading, spacing: 12) {
                FormLine(title: "Provider") {
                    Text(providerName)
                        .foregroundStyle(.secondary)
                }
                FormLine(title: "入口 ID") {
                    if let row {
                        Text(row.id)
                            .font(.callout.monospaced())
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    } else {
                        TextField("留空则按名称自动生成", text: $idText)
                    }
                }
                FormLine(title: "名称") {
                    TextField("名称", text: $name)
                }
                FormLine(title: "API 地址") {
                    TextField("https://api.example.com", text: $baseURL)
                }
                FormLine(title: "API Key") {
                    VStack(alignment: .leading, spacing: 2) {
                        RevealableSecureField(placeholder: "留空 = 无鉴权转发", text: $apiKey)
                        Text(row == nil
                            ? "留空则不发鉴权头(适合本地/内网无鉴权上游);是否需要鉴权请按上游服务要求配置。"
                            : "已保存的 Key 已经填在这里,点右侧眼睛看完整值;清空再保存 = 该入口改为无鉴权转发。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                FormLine(title: "入口协议") {
                    VStack(alignment: .leading, spacing: 2) {
                        Picker("入口协议", selection: $protocolName) {
                            ForEach(EndpointProtocolMode.allCases, id: \.rawValue) { item in
                                Text(item.displayName).tag(item.rawValue)
                            }
                        }
                        .labelsHidden()
                        .frame(width: 190, alignment: .leading)
                        Text("自动会按请求入口选择 Anthropic、OpenAI Chat 或 Responses 原生协议；固定协议仅用于该上游只支持一种协议时。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                FormLine(title: "启用") {
                    Toggle("启用", isOn: $enabled)
                        .labelsHidden()
                }
                FormLine(title: "优先级") {
                    VStack(alignment: .leading, spacing: 2) {
                        TextField("0", text: $priorityText)
                            .frame(width: 100)
                        Text("数值越小越优先；同级按入口配置顺序。新入口默认 0。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Divider()
                FormLine(title: "Pinned IPs") {
                    VStack(alignment: .leading, spacing: 2) {
                        TextField("可空，逗号分隔", text: $pinnedIPs)
                        Text("填了就会优先按这些 IP 直连（TLS SNI 仍是域名）；是否回落 DNS 由下面的开关决定。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                FormLine(title: "源站直连") {
                    VStack(alignment: .leading, spacing: 2) {
                        Toggle("启用 IP pin", isOn: $pin)
                            .labelsHidden()
                        Text(pin && pinnedIPs.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                            ? "没填 IP，仍会走域名解析。"
                            : "开启 = 只走上面的 IP，不回落 DNS；关闭 = IP 与 DNS 一起竞速。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                FormLine(title: "粘性分组") {
                    VStack(alignment: .leading, spacing: 2) {
                        TextField("留空则使用入口 ID 作为独立粘性组", text: $stickyGroup)
                        Text("留空入口也参与统一 Provider 分流；同组入口共享会话粘性，传输失败不会建立账号级冷却。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                FormLine(title: "连接复用") {
                    VStack(alignment: .leading, spacing: 2) {
                        Toggle("启用 Keep-Alive", isOn: $keepAlive)
                            .labelsHidden()
                        Text("新入口默认启用；可按入口关闭。启用后复用该入口的出站连接，减少重复 TCP/TLS 握手。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                FormLine(title: "成本价格") {
                    VStack(alignment: .leading, spacing: 6) {
                        Button {
                            showPricingEditor = true
                        } label: {
                            Label("编辑此入口价格", systemImage: "yensign.circle")
                        }
                        .disabled(row == nil)
                        .frame(minHeight: 44)
                        Text(row == nil
                            ? "请先保存入口，再为该入口维护输入、输出和缓存价格。"
                            : "价格按此入口的模型分别维护；未配置专属价格时使用全局回退。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                SheetErrorText(submission.errorText)
            }
        }
        .sheet(isPresented: $showPricingEditor) {
            RuntimePricingEditor(model: model, endpointID: row?.id)
        }
    }

    // 空 API Key 合法(= 无鉴权转发),不再拦提交。
    private var canSubmit: Bool {
        !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
            !baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func submit() {
        guard submission.begin() else { return }
        Task {
            do {
                let priority = try parsePriority(priorityText)
                if let row {
                    try await model.updateProviderAccount(
                        id: row.id,
                        name: name,
                        baseURLText: baseURL,
                        protocolName: protocolName,
                        enabled: enabled,
                        apiKey: apiKey,
                        pinnedIPsText: pinnedIPs,
                        priority: priority,
                        pin: pin,
                        stickyGroup: stickyGroup,
                        keepAlive: keepAlive
                    )
                } else {
                    try await model.addProviderAccount(
                        idText: idText,
                        name: name,
                        baseURLText: baseURL,
                        protocolName: protocolName,
                        enabled: enabled,
                        apiKey: apiKey,
                        pinnedIPsText: pinnedIPs,
                        priority: priority,
                        pin: pin,
                        stickyGroup: stickyGroup,
                        keepAlive: keepAlive
                    )
                }
                submission.succeed()
                onClose()
            } catch {
                submission.fail(error.localizedDescription)
            }
        }
    }

    private func parsePriority(_ text: String) throws -> Int {
        let cleaned = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let value = Int(cleaned), value >= 0 else {
            throw AppModelError.invalidInput("优先级必须是非负整数")
        }
        return value
    }
}

struct SheetErrorText: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        if !text.isEmpty {
            Text(text)
                .font(.caption)
                .foregroundStyle(.red)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.leading, 122)
        }
    }
}
