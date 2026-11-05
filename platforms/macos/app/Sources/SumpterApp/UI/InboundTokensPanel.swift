import SwiftUI
import Security
import SumpterCore

struct InboundTokensPanel: View {
    @ObservedObject var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @State private var entries: [InboundTokenConfig] = []
    @State private var baseline: ListenerConfig?
    @State private var saving = false
    @State private var error: String?
    @State private var confirmOpen = false
    @State private var confirmReload = false
    @State private var revealed: Set<String> = []
    @State private var editing: Set<String> = []

    private var dirty: Bool { baseline.map { entries != $0.editableInboundTokens } ?? false }

    private func load() {
        baseline = model.config.listener
        entries = model.config.listener.editableInboundTokens
        revealed = []; editing = []; error = nil
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
                try next.setEditableInboundTokens(entries)
                let saved = next
                try await model.mutateConfig { config in
                    guard config.listener == baseline else { throw AppModelError.invalidInput("监听配置已变化，请重新载入 Token 列表后重试") }
                    config.listener = saved
                }
                load()
                model.flash("Token 已保存")
            } catch { self.error = error.localizedDescription }
            saving = false
        }
    }

    var body: some View {
        SectionPanel(title: "入站 API Token", hint: "默认 Token 与新增 Token 统一管理，可显示、复制、改名或修改凭据；统计归属保持不变。") {
            VStack(alignment: .leading, spacing: 12) {
                ForEach($entries) { $entry in
                    VStack(alignment: .leading, spacing: 10) {
                        HStack {
                            if entry.id == "legacy" { Text("默认 Token").font(.caption).foregroundStyle(.secondary) }
                            StatusBadge(text: entry.enabled ? "已启用" : "已停用", systemImage: entry.enabled ? "lock.fill" : "pause.circle", color: entry.enabled ? .green : .secondary)
                            Spacer()
                            Toggle("启用", isOn: $entry.enabled)
                        }
                        FormLine(title: "名称") { TextField("例如：工作电脑", text: $entry.name) }
                        FormLine(title: "Token") {
                            if editing.contains(entry.id) {
                                RevealableSecureField(placeholder: "输入 Token", text: $entry.token)
                            } else {
                                Text(revealed.contains(entry.id) ? entry.token : "•••••••• · 尾号 \(String(entry.token.suffix(4)))")
                                    .font(.system(.body, design: .monospaced)).textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                        }
                        ViewThatFits(in: .horizontal) {
                            HStack { tokenActions($entry) }
                            VStack(alignment: .leading) { tokenActions($entry) }
                        }
                    }
                    Divider()
                }
                if entries.isEmpty { Text("暂无入站 Token，点击新增开始配置。").foregroundStyle(.secondary) }
                Text("已保存凭据显示后 60 秒自动隐藏。全部停用会拒绝请求；删除保留历史统计。").font(.caption).foregroundStyle(.secondary)
                if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
                HStack {
                    Button("新增 Token") {
                        if let token = generatedToken() {
                            let entry = InboundTokenConfig(name: "", token: token)
                            entries.append(entry); editing.insert(entry.id)
                        }
                    }.disabled(entries.filter { $0.id != "legacy" }.count >= 128)
                    Button("重新载入") { if dirty { confirmReload = true } else { load() } }
                    Spacer()
                    Button(saving ? "保存中…" : "保存修改") {
                        if entries.isEmpty && baseline?.hasInboundAuth == true { confirmOpen = true }
                        else { save() }
                    }.disabled(!dirty)
                }
            }.disabled(saving)
        }
        .onAppear { load() }
        .onChange(of: model.config.listener) { _, _ in
            revealed = []
            if !dirty && !saving { load() }
        }
        .onChange(of: scenePhase) { _, phase in if phase != .active { revealed = [] } }
        .task(id: revealed) {
            guard !revealed.isEmpty else { return }
            do { try await Task.sleep(for: .seconds(60)); revealed = [] } catch {}
        }
        .confirmationDialog("删除所有 Token 后将关闭入站认证。确认保存？", isPresented: $confirmOpen) {
            Button("保存", role: .destructive) { save() }
            Button("取消", role: .cancel) {}
        }
        .confirmationDialog("放弃未保存的修改并重新载入？", isPresented: $confirmReload) {
            Button("重新载入", role: .destructive) { load() }
            Button("取消", role: .cancel) {}
        }
    }

    @ViewBuilder
    private func tokenActions(_ entry: Binding<InboundTokenConfig>) -> some View {
        let id = entry.wrappedValue.id
        Button(revealed.contains(id) ? "隐藏" : "显示") {
            if revealed.contains(id) { revealed.remove(id) } else { revealed.insert(id) }
        }.disabled(editing.contains(id))
        Button("复制") {
            if PasteboardCopy.write(entry.wrappedValue.token) { model.flash("Token 已复制") }
            else { error = "复制失败，请重试" }
        }
        Button("修改 Token") { editing.insert(id) }.disabled(editing.contains(id))
        Button("生成新凭据") { if let token = generatedToken() { entry.wrappedValue.token = token; editing.insert(id) } }
        Button("删除", role: .destructive) { entries.removeAll { $0.id == id }; revealed.remove(id); editing.remove(id) }
    }
}
