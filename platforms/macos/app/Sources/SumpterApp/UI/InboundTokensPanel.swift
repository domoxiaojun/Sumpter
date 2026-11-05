import SwiftUI
import Security
import SumpterCore

struct InboundTokensPanel: View {
    @ObservedObject var model: AppModel
    @State private var entries: [InboundTokenConfig] = []
    @State private var legacyName = "默认 Token"
    @State private var baseline: ListenerConfig?
    @State private var saving = false
    @State private var error: String?
    @State private var confirmOpen = false

    private var dirty: Bool {
        guard let baseline else { return false }
        return legacyName != baseline.authTokenName || entries != baseline.authTokens.map { entry in
            var copy = entry; copy.token = ""; return copy
        }
    }

    private func load() {
        baseline = model.config.listener
        legacyName = model.config.listener.authTokenName
        entries = model.config.listener.authTokens.map { entry in
            var copy = entry; copy.token = ""; return copy
        }
    }

    private func generatedToken() -> String? {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
            error = "无法生成随机 Token，请重试"; return nil
        }
        return "sk-sumpter-" + bytes.map { String(format: "%02x", $0) }.joined()
    }

    private func save() {
        guard let baseline else { return }
        saving = true; error = nil
        Task {
            do {
                var next = baseline
                next.authTokenName = legacyName.trimmingCharacters(in: .whitespacesAndNewlines)
                next.authTokens = entries.map { entry in
                    var copy = entry
                    copy.name = copy.name.trimmingCharacters(in: .whitespacesAndNewlines)
                    if copy.token.isEmpty { copy.token = baseline.authTokens.first(where: { $0.id == copy.id })?.token ?? "" }
                    return copy
                }
                try next.validateInboundTokens()
                let saved = next
                try await model.mutateConfig { config in
                    guard config.listener == baseline else { throw AppModelError.invalidInput("监听配置已变化，请重新载入 Token 列表后重试") }
                    config.listener = saved
                }
                load()
                model.flash("Token 列表已保存")
            } catch { self.error = error.localizedDescription }
            saving = false
        }
    }

    var body: some View {
        SectionPanel(title: "多个入站客户端 API Token", hint: "按名称区分客户端用量。新凭据保存后只显示尾号，请先复制。未配置默认 Token 时，全部停用会拒绝请求；删除保留历史统计。") {
            VStack(alignment: .leading, spacing: 12) {
                if !model.config.listener.authToken.isEmpty {
                    FormLine(title: "默认 Token 名称") { TextField("名称", text: $legacyName) }
                }
                ForEach($entries) { $entry in
                    VStack(alignment: .leading, spacing: 8) {
                        FormLine(title: "名称") { TextField("例如：工作电脑", text: $entry.name) }
                        FormLine(title: "Token") { RevealableSecureField(placeholder: "留空保留原凭据", text: $entry.token) }
                        if let old = baseline?.authTokens.first(where: { $0.id == entry.id }) {
                            Text("现有凭据尾号：\(String(old.token.suffix(4)))").font(.caption).foregroundStyle(.secondary)
                        }
                        HStack {
                            Toggle("启用", isOn: $entry.enabled)
                            Spacer()
                            Button("生成新凭据") { if let token = generatedToken() { entry.token = token } }
                            Button("删除", role: .destructive) { entries.removeAll { $0.id == entry.id } }
                        }
                    }
                    Divider()
                }
                if entries.isEmpty { Text("尚未添加命名 Token。").foregroundStyle(.secondary) }
                if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
                HStack {
                    Button("新增 Token") {
                        if let token = generatedToken() { entries.append(InboundTokenConfig(name: "", token: token)) }
                    }.disabled(entries.count >= 128)
                    Button("重新载入") { load(); error = nil }
                    Spacer()
                    Button(saving ? "保存中…" : "保存 Token 列表") {
                        if entries.isEmpty && baseline?.authToken.isEmpty == true && baseline?.authTokens.isEmpty == false { confirmOpen = true }
                        else { save() }
                    }.disabled(!dirty)
                }
            }.disabled(saving)
        }
        .onAppear { load() }
        .onChange(of: model.config.listener) { _, _ in if !dirty && !saving { load() } }
        .confirmationDialog("删除最后一个 Token 后将关闭入站认证。确认保存？", isPresented: $confirmOpen) {
            Button("保存", role: .destructive) { save() }
            Button("取消", role: .cancel) {}
        }
    }
}
