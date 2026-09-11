import SwiftUI
import SumpterCore

struct ModelGroupBindingEditor: View {
    @Binding var binding: ModelGroupBinding
    let models: [String]
    let endpoint: Endpoint?
    let canMoveUp: Bool; let canMoveDown: Bool; let up: () -> Void; let down: () -> Void; let remove: () -> Void
    /// 默认组的顺序与优先级保存时跟随入口库(mutateConfig 同步),编辑器
    /// 对这两项显示只读口径,避免给出会被覆盖的假开关。
    var followsLibrary = false
    @State var expanded = false
    @State private var search = ""
    @Environment(\.sumpterPalette) private var palette

    private var available: [String] { ModelGroupCatalog.availableModels(endpoint: endpoint, groupModels: models) }
    private var unavailable: [String] { (binding.models ?? []).filter { !available.contains($0) } }
    private var rowModels: [String] { available }
    private var selectedModels: [String] {
        available.filter { name in binding.models?.contains { ModelName.matches(name, pattern: $0) } ?? true }
    }
    private var visibleModels: [String] {
        let query = search.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty ? rowModels : rowModels.filter { $0.localizedCaseInsensitiveContains(query) }
    }
    private var enabledText: String {
        if endpoint?.enabled == false { return "入口库已停用" }
        return binding.enabled ? "组内启用" : "组内停用"
    }
    private var scopeSummary: String {
        binding.models == nil ? "全部可用模型 · 可选 \(available.count)" : "已选 \(selectedModels.count) · 可选 \(available.count)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Button { expanded.toggle() } label: {
                    HStack(spacing: 10) {
                        Image(systemName: "server.rack").font(.system(size: 15)).foregroundStyle(palette.brand)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(endpoint?.name ?? binding.endpointID).font(.system(size: 13, weight: .semibold))
                            Text("\(scopeSummary) · \(followsLibrary ? "优先级跟随入口库" : "优先级 \(binding.priority)")")
                                .font(.caption).foregroundStyle(palette.textSecondary)
                        }
                        Spacer(minLength: 0)
                        Label(enabledText, systemImage: endpoint?.enabled == false ? "pause.circle" : binding.enabled ? "checkmark.circle" : "pause.circle")
                            .font(.caption).foregroundStyle(endpoint?.enabled == false ? palette.warning : palette.textSecondary)
                        Image(systemName: expanded ? "chevron.up" : "chevron.down").font(.caption)
                    }
                    .padding(.horizontal, 10).padding(.vertical, 7)
                    .frame(maxWidth: .infinity, minHeight: 46, alignment: .leading).contentShape(Rectangle())
                }
                .buttonStyle(ModelGroupTileStyle(borderless: true))
                .accessibilityLabel("编辑入口 \(endpoint?.name ?? binding.endpointID)")
                .accessibilityValue(expanded ? "已展开" : "已收起")
                if !followsLibrary {
                    HStack(spacing: 4) {
                        Button(action: up) { Image(systemName: "arrow.up").frame(minWidth: 20, minHeight: 24) }
                            .disabled(!canMoveUp).help("入口上移").accessibilityLabel("入口上移")
                        Button(action: down) { Image(systemName: "arrow.down").frame(minWidth: 20, minHeight: 24) }
                            .disabled(!canMoveDown).help("入口下移").accessibilityLabel("入口下移")
                    }.padding(.trailing, 8)
                }
            }
            if expanded {
                Divider().padding(.horizontal, 10)
                editor.padding(10)
            }
        }
        .font(.system(size: 13)).controlSize(.regular)
        .background(palette.surface, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(palette.borderMedium))
    }

    private var editor: some View {
        VStack(alignment: .leading, spacing: 10) {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 160, maximum: 260), spacing: 16)], alignment: .leading, spacing: 8) {
                setting("组内状态") { Toggle("在此组中启用", isOn: $binding.enabled).toggleStyle(.checkbox) }
                setting("默认优先级") {
                    if followsLibrary {
                        Text("跟随入口库")
                            .font(.system(size: 13)).foregroundStyle(palette.textSecondary)
                            .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                            .help("默认组的顺序与优先级保存时自动跟随入口库")
                    } else {
                        TextField("入口优先级", value: $binding.priority, format: .number)
                            .textFieldStyle(.roundedBorder).frame(maxWidth: 100).help("数字越小越优先")
                    }
                }
                setting("承接范围") {
                    Picker("承接范围", selection: Binding(
                        get: { binding.models == nil },
                        set: { binding.models = $0 ? nil : available; pruneOverrides() }
                    )) { Text("指定模型").tag(false); Text("全部可用模型").tag(true) }
                        .labelsHidden().frame(maxWidth: .infinity, alignment: .leading)
                }
            }

            if endpoint?.enabled == false {
                Label("入口库已停用，此处设置保留，启用入口后才会参与调度。", systemImage: "pause.circle")
                    .font(.caption).foregroundStyle(palette.warning)
            }
            if binding.models == nil {
                Label("仅承接本入口已添加的组内模型；新增可用模型会自动纳入，移除入口模型后立即停止承接新请求。", systemImage: "info.circle")
                    .font(.caption).foregroundStyle(palette.textSecondary)
            }

            VStack(alignment: .leading, spacing: 6) {
                ViewThatFits(in: .horizontal) {
                    HStack { listHeading; Spacer(); selectionActions }
                    VStack(alignment: .leading, spacing: 8) { listHeading; selectionActions }
                }
                if rowModels.count > 8 || !search.isEmpty {
                    TextField("搜索入口模型", text: $search).textFieldStyle(.roundedBorder).frame(maxWidth: 320)
                }
                ViewThatFits(in: .horizontal) {
                    modelTable(stacked: false).frame(minWidth: 620)
                    modelTable(stacked: true)
                }
            }
            HStack {
                if !unavailable.isEmpty {
                    Label("\(unavailable.count) 项历史选择不在当前可选范围，已从列表隐藏。", systemImage: "exclamationmark.triangle")
                        .font(.caption).foregroundStyle(palette.warning)
                    Button("清理历史选择") { binding.models = selectedModels; pruneOverrides() }
                }
                Spacer()
                Button("移出组", role: .destructive, action: remove)
            }
        }
    }

    private var listHeading: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text("模型与覆盖").font(.system(size: 13, weight: .medium))
                Text(scopeSummary).font(.caption).foregroundStyle(palette.textSecondary)
            }
            Text("覆盖留空时继承入口设置。")
                .font(.caption).foregroundStyle(palette.textSecondary)
        }
    }
    private var selectionActions: some View {
        HStack(spacing: 8) {
            Button("全选可用") { binding.models = available; pruneOverrides() }
            Button("清空") { binding.models = []; binding.overrides = [] }
        }
    }
    private func setting<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).font(.caption).foregroundStyle(palette.textSecondary)
            content().frame(minHeight: 28, alignment: .leading)
        }
    }

    private func modelTable(stacked: Bool) -> some View {
        VStack(spacing: 0) {
            if !stacked {
                HStack(spacing: 16) {
                    Text("模型").frame(minWidth: 200, maxWidth: .infinity, alignment: .leading)
                    HStack(spacing: 10) {
                        Text("上游模型名称").frame(maxWidth: .infinity, alignment: .leading)
                        Text("优先级").frame(width: 80, alignment: .leading)
                        Color.clear.frame(width: 40, height: 1)
                    }.frame(minWidth: 300, maxWidth: .infinity)
                }.font(.system(size: 12)).foregroundStyle(palette.textSecondary).padding(.horizontal, 8).padding(.vertical, 6)
                Divider()
            }
            ForEach(visibleModels, id: \.self) { name in
                if name != visibleModels.first { Divider() }
                modelRow(name, stacked: stacked).padding(.horizontal, 8).padding(.vertical, 5)
            }
            if visibleModels.isEmpty {
                Text(rowModels.isEmpty ? "暂无可选模型，请先在入口库添加模型并纳入当前组。" : "没有匹配的模型。")
                    .foregroundStyle(palette.textSecondary).frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).padding(8)
            }
        }.background(palette.inset, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(palette.borderSubtle))
    }
    @ViewBuilder private func modelRow(_ name: String, stacked: Bool) -> some View {
        let selected = binding.models?.contains { ModelName.matches(name, pattern: $0) } ?? true
        let inherited = binding.models != nil && selected && !(binding.models ?? []).contains(name)
        if stacked {
            VStack(alignment: .leading, spacing: 6) {
                modelSelection(name, selected: selected, inherited: inherited)
                rowFields(name, selected: selected, showLabels: true)
            }
        } else {
            HStack(spacing: 16) {
                modelSelection(name, selected: selected, inherited: inherited).frame(minWidth: 200, maxWidth: .infinity)
                rowFields(name, selected: selected, showLabels: false).frame(minWidth: 300, maxWidth: .infinity)
            }
        }
    }
    private func modelSelection(_ name: String, selected: Bool, inherited: Bool) -> some View {
        Toggle(isOn: Binding(get: { selected }, set: { checked in
            guard binding.models != nil else { return }
            if checked { binding.models = Array(Set((binding.models ?? []) + [name])).sorted() }
            else { binding.models?.removeAll { $0 == name }; pruneOverrides() }
        })) {
            VStack(alignment: .leading, spacing: 4) {
                Text(name).font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
                if inherited { Text("由已选通配符承接").font(.caption).foregroundStyle(palette.textSecondary) }
                else if !available.contains(name) { Text("未在此入口添加").font(.caption).foregroundStyle(palette.warning) }
                else if binding.overrides.contains(where: { $0.model == name }) {
                    Text("已设置覆盖").font(.caption).foregroundStyle(palette.brand)
                }
            }
        }
        .toggleStyle(BindingModelToggleStyle())
        .disabled(binding.models == nil || inherited)
    }
    @ViewBuilder private func rowFields(_ name: String, selected: Bool, showLabels: Bool) -> some View {
        if name.contains("*") {
            Text("通配符沿用入口映射；具体模型可设置单独覆盖。")
                .font(.caption).foregroundStyle(palette.textSecondary).frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        } else if !selected {
            Text("选中后可设置上游名称与优先级")
                .font(.caption).foregroundStyle(palette.textMuted).frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        } else {
            HStack(alignment: .bottom, spacing: 10) {
                VStack(alignment: .leading, spacing: 3) {
                    if showLabels { Text("上游模型名称").font(.caption).foregroundStyle(palette.textSecondary) }
                    TextField("继承入口映射", text: overrideText(name, priority: false))
                        .textFieldStyle(.roundedBorder).accessibilityLabel("\(name) 上游模型名称")
                }.frame(maxWidth: .infinity)
                VStack(alignment: .leading, spacing: 3) {
                    if showLabels { Text("优先级").font(.caption).foregroundStyle(palette.textSecondary) }
                    TextField("继承 \(binding.priority)", text: overrideText(name, priority: true))
                        .textFieldStyle(.roundedBorder).accessibilityLabel("\(name) 优先级")
                }.frame(width: 80)
                Button {
                    binding.overrides.removeAll { $0.model == name }
                } label: { Image(systemName: "arrow.counterclockwise").frame(width: 20, height: 24) }
                    .frame(width: 40)
                    .help("恢复继承").accessibilityLabel("\(name) 恢复继承")
                    .disabled(!binding.overrides.contains { $0.model == name })
            }
        }
    }
    private func pruneOverrides() {
        guard let selected = binding.models else { return }
        binding.overrides.removeAll { item in !selected.contains { ModelName.matches(item.model, pattern: $0) } }
    }
    private func overrideText(_ name: String, priority: Bool) -> Binding<String> {
        Binding(get: {
            let o = binding.overrides.first { $0.model == name }
            return priority ? o?.priority.map(String.init) ?? "" : o?.upstreamModel ?? ""
        }, set: { value in
            if !binding.overrides.contains(where: { $0.model == name }) { binding.overrides.append(ModelGroupModelOverride(model: name)) }
            guard let index = binding.overrides.firstIndex(where: { $0.model == name }) else { return }
            if priority { binding.overrides[index].priority = Int(value) }
            else { binding.overrides[index].upstreamModel = value.isEmpty ? nil : value }
            binding.overrides.removeAll { $0.priority == nil && $0.upstreamModel == nil }
        })
    }
}

private struct BindingModelToggleStyle: ToggleStyle {
    @Environment(\.sumpterPalette) private var palette
    func makeBody(configuration: Configuration) -> some View {
        Button { configuration.isOn.toggle() } label: {
            HStack(spacing: 10) {
                Image(systemName: configuration.isOn ? "checkmark.square.fill" : "square")
                    .font(.system(size: 16)).foregroundStyle(configuration.isOn ? palette.brand : palette.textMuted)
                configuration.label.multilineTextAlignment(.leading)
                Spacer(minLength: 0)
            }.frame(maxWidth: .infinity, minHeight: 32, alignment: .leading).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityValue(configuration.isOn ? "已选择" : "未选择")
    }
}
