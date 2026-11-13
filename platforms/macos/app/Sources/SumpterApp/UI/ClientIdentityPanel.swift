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
        SectionPanel(title:"客户端识别", hint:"通常保持自动识别即可。仅为缺少产品标记的客户端绑定独立 Token；多个客户端共用 Token 时请留空。原生标记始终优先。") {
            VStack(alignment:.leading, spacing:12) {
                Text("自定义标识（如 my-app）仅补充产品标签，不会新增识别规则，也不会补出项目、会话或主代理身份。").font(.caption).foregroundStyle(.secondary)
                if model.config.listener.editableInboundTokens.isEmpty {
                    Text("尚未配置入站 Token。客户端仍按原生标记识别；需要兜底绑定时，先添加独立 Token。").font(.caption)
                }
                ForEach(model.config.listener.editableInboundTokens) { token in
                    HStack {
                        Text(token.name + (token.enabled ? "" : "（已停用）")).font(.headline)
                        Spacer()
                        if draft.tokenBindings[token.id] != nil {
                            Button("恢复自动识别") { draft.tokenBindings.removeValue(forKey:token.id) }
                        }
                    }
                    FormLine(title:"客户端标识（可选）") { TextField("自动识别（留空），例如 codex", text:binding(token.id,"client")) }
                    FormLine(title:"客户端形态（可选）") {
                        Picker("客户端形态", selection:binding(token.id,"variant")) {
                            Text("自动识别").tag("")
                            ForEach(ClientRegistry.variants, id:\.self) { Text($0).tag($0) }
                        }.labelsHidden().disabled(draft.tokenBindings[token.id]?.client.isEmpty ?? true)
                    }
                    Text("先填写客户端标识，再选择 cli / desktop 等形态；这不是代理上游入口。").font(.caption).foregroundStyle(.secondary)
                    DisclosureGroup("受信中继设置" + (draft.tokenBindings[token.id]?.relaySource == nil ? "（普通客户端无需设置）" : "（已配置）")) {
                        FormLine(title:"受信中继来源（可选）") {
                            TextField("例如 office", text:binding(token.id,"relaySource"))
                                .disabled(draft.tokenBindings[token.id]?.client.isEmpty ?? true)
                        }
                        Text("仅用于另一台 Sumpter 的专用 Token，先填写客户端标识（如 sumpter）。持有此 Token 的发送方可声明原始客户端，来源须与发送方一致；请勿与普通客户端共用。").font(.caption).foregroundStyle(.secondary)
                    }
                    Divider()
                }
                DisclosureGroup("高级设置：统计映射、UA 别名与中继目标") {
                    VStack(alignment:.leading, spacing:12) {
                        Text("统计归属映射（每行 产品标识=客户端ID）")
                        TextEditor(text:$reporting).font(.body.monospaced()).frame(minHeight:70)
                        Text("例如 my-app=codex，将 my-app 计入 Codex。仅影响查询时的统计分组与筛选；删除可恢复，历史原始身份、权限和路由保持不变。").font(.caption)
                        Text("产品 UA 精确别名（每行 UA产品标识=客户端ID）")
                        TextEditor(text:$ua).font(.body.monospaced()).frame(minHeight:70)
                        Text("例如 custom-cli=open_code，仅匹配完整产品名 custom-cli/1.0 中的 custom-cli。用于后续请求，不支持正则，也不覆盖已识别的原生客户端。").font(.caption)
                        Text("中继目标（每行 入口ID=来源标识）")
                        TextEditor(text:$relays).font(.body.monospaced()).frame(minHeight:60)
                        Text("仅填写另一台 Sumpter 的上游入口，并与接收方“受信中继来源”一致；普通供应商入口留空。").font(.caption)
                    }
                }
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
