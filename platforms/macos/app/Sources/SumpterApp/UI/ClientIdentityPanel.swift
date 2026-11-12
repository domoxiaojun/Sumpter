import SwiftUI
import SumpterCore

struct ClientIdentityPanel: View {
    @ObservedObject var model: AppModel
    @State private var draft = ClientIdentitySettings()
    @State private var baseline: ClientIdentitySettings?
    @State private var reporting = ""
    @State private var ua = ""
    @State private var relays = ""
    @State private var busy = false
    @State private var error: String?
    private var dirty: Bool { baseline != nil && (draft != baseline || reporting != ClientIdentitySettings.aliasesText(baseline?.reportingAliases ?? [:]) || ua != ClientIdentitySettings.aliasesText(baseline?.uaAliases ?? [:]) || relays != ClientIdentitySettings.aliasesText(baseline?.relayEndpoints ?? [:])) }
    private func load() {
        draft = model.config.listener.clientIdentity ?? .init(); baseline = draft
        reporting = ClientIdentitySettings.aliasesText(draft.reportingAliases)
        ua = ClientIdentitySettings.aliasesText(draft.uaAliases)
        relays = ClientIdentitySettings.aliasesText(draft.relayEndpoints)
    }
    private func binding(_ id:String, _ field:String) -> Binding<String> {
        Binding(get: { let b = draft.tokenBindings[id]; return field == "client" ? b?.client ?? "" : field == "variant" ? b?.variant ?? "" : b?.relaySource ?? "" }, set: { value in
            var b = draft.tokenBindings[id] ?? .init()
            if field == "client" { b.client = value } else if field == "variant" { b.variant = value.isEmpty ? nil : value } else { b.relaySource = value.isEmpty ? nil : value }
            if b.client.isEmpty { draft.tokenBindings.removeValue(forKey:id) } else { draft.tokenBindings[id] = b }
        })
    }
    private func save() {
        busy = true; error = nil
        Task {
            do {
                var saved = draft
                saved.reportingAliases = try ClientIdentitySettings.parseAliases(reporting)
                saved.uaAliases = try ClientIdentitySettings.parseAliases(ua)
                saved.relayEndpoints = try ClientIdentitySettings.parseAliases(relays, catalog:false)
                try saved.validate()
                try await model.mutateConfig { config in
                    guard (config.listener.clientIdentity ?? .init()) == baseline else { throw AppModelError.invalidInput("客户端设置已变化，请重新载入") }
                    config.listener.clientIdentity = saved
                }
                load(); model.flash("客户端识别设置已保存")
            } catch { self.error = error.localizedDescription }
            busy = false
        }
    }
    var body: some View {
        SectionPanel(title:"客户端识别", hint:"原生标记优先，Token 绑定只补足缺少的身份；不会补出项目或会话。") {
            VStack(alignment:.leading, spacing:12) {
                ForEach(model.config.listener.editableInboundTokens) { token in
                    Text(token.name).font(.headline)
                    FormLine(title:"客户端标识") { TextField("自动识别", text:binding(token.id,"client")) }
                    FormLine(title:"入口") { TextField("自动识别，例如 cli / desktop", text:binding(token.id,"variant")) }
                    FormLine(title:"受信中继来源") { TextField("普通客户端留空", text:binding(token.id,"relaySource")) }
                    Divider()
                }
                Text("统计归属映射（每行 产品标识=客户端ID）")
                TextEditor(text:$reporting).font(.body.monospaced()).frame(minHeight:70)
                Text("只改变统计分组，删除映射即可恢复；原始身份、权限和路由不变。").font(.caption)
                Text("产品 UA 精确别名")
                TextEditor(text:$ua).font(.body.monospaced()).frame(minHeight:70)
                Text("中继目标（每行 入口ID=来源标识，仅填写另一台 Sumpter 的入口）")
                TextEditor(text:$relays).font(.body.monospaced()).frame(minHeight:60)
                DisclosureGroup("原生识别能力目录") {
                    ForEach(ClientRegistry.catalog, id:\.id) { client in
                        VStack(alignment:.leading) {
                            Text("\(client.name) · \(client.id)")
                            Text("会话：\(client.nativeSessionHeaders.isEmpty ? "按请求证据" : "支持") · 原生角色：\(client.nativeRoles ? "支持" : "无专用标记") · 原生用途：\(client.nativeTasks ? "支持" : "无专用标记")").font(.caption)
                        }.textSelection(.enabled)
                    }
                }
                if let error { Text(error).foregroundStyle(.red) }
                HStack { Button("重新载入") { load() }; Spacer(); Button(busy ? "保存中…" : "保存客户端识别") { save() }.disabled(!dirty) }
            }.disabled(busy)
        }.onAppear { load() }.onChange(of:model.config.listener.clientIdentity) { _,_ in if !dirty && !busy { load() } }
    }
}
