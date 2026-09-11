import SwiftUI
import SumpterCore

struct ModelGroupsPane: View {
    @ObservedObject var model: AppModel
    @State private var groups: [ModelGroup] = []
    @State private var original: [ModelGroup] = []
    @State private var selectedID: String?
    @State private var saving = false
    @State private var error: String?
    @State private var deleteID: String?
    private var dirty: Bool { groups != original }
    @Environment(\.sumpterPalette) private var palette

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let error { Text(error).foregroundStyle(palette.danger).textSelection(.enabled) }
                ModelGroupsWorkspace(groups: $groups, selectedID: $selectedID, endpoints: model.config.endpoints,
                    dirty: dirty, saving: saving, canSave: dirty || model.config.modelGroups == nil,
                    save: save, revert: { groups = original }, delete: { deleteID = $0 })
            }
            .padding(16).frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .background(palette.canvas)
        .onAppear { load() }
        .onChange(of: model.config) { _, _ in if !dirty { load() } }
        .alert("删除模型组？", isPresented: Binding(get: { deleteID != nil }, set: { if !$0 { deleteID = nil } })) {
            Button("删除", role: .destructive) { groups.removeAll { $0.id == deleteID }; deleteID = nil }
            Button("取消", role: .cancel) { deleteID = nil }
        } message: { Text("入口库中的连接配置会保留。保存更改后生效。") }
    }

    private func load() {
        var config = model.config; config.migrateModelGroups()
        groups = config.modelGroups ?? []; original = groups
        if !groups.contains(where: { $0.id == selectedID }) { selectedID = groups.first?.id }
    }
    private func save() {
        saving = true; error = nil; let draftGroups = groups
        Task {
            do { try await model.mutateConfig { config in config.modelGroups = draftGroups }; load(); model.flash("模型组已保存，客户端地址保持不变") }
            catch { self.error = String(describing: error) }
            saving = false
        }
    }
}

struct ModelGroupsWorkspace: View {
    @Binding var groups: [ModelGroup]
    @Binding var selectedID: String?
    let endpoints: [Endpoint]
    var dirty = false
    var saving = false
    var canSave = false
    let save: () -> Void
    let revert: () -> Void
    let delete: (String) -> Void
    @State private var tab = "models"
    private var selectedIndex: Int? { groups.firstIndex { $0.id == selectedID } ?? groups.indices.first }
    @Environment(\.sumpterPalette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ViewThatFits(in: .horizontal) {
                HStack { heading; Spacer(minLength: 16); actions }
                VStack(alignment: .leading, spacing: 8) { heading; actions }
            }
            if groups.isEmpty {
                Text("尚无模型组。新建组后选择模型，再从入口库添加入口。")
                    .foregroundStyle(palette.textSecondary).padding(.vertical, 16)
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 190, maximum: 280), spacing: 8)], alignment: .leading, spacing: 8) {
                        ForEach(Array(groups.enumerated()), id: \.element.id) { index, group in
                            Button { selectedID = group.id } label: {
                                HStack(spacing: 8) {
                                    VStack(alignment: .leading, spacing: 3) {
                                        Text(group.name.isEmpty ? "未命名模型组" : group.name).fontWeight(.semibold)
                                        Text("\(group.models.count) 模型 · \(group.bindings.count) 入口" + (group.enabled ? "" : " · 停用"))
                                            .font(.system(size: 12)).foregroundStyle(palette.textSecondary)
                                    }
                                    Spacer(minLength: 0)
                                    if index == selectedIndex { Image(systemName: "checkmark").foregroundStyle(palette.brand) }
                                }.padding(.horizontal, 10).padding(.vertical, 7)
                                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                            }
                            .buttonStyle(ModelGroupTileStyle(selected: index == selectedIndex))
                            .accessibilityValue(index == selectedIndex ? "当前模型组" : "切换模型组")
                        }
                    }
                    Divider()
                    if let index = selectedIndex {
                        ModelGroupSettings(group: $groups[index], canMoveUp: index > 0, canMoveDown: index + 1 < groups.count,
                            moveUp: { groups.swapAt(index, index - 1) }, moveDown: { groups.swapAt(index, index + 1) },
                            delete: { delete(groups[index].id) })
                    }
                }.padding(12).background(palette.surface, in: RoundedRectangle(cornerRadius: 12))
                    .overlay(RoundedRectangle(cornerRadius: 12).stroke(palette.borderSubtle))
                    .disabled(saving)
            }
            if let index = selectedIndex {
                ModelGroupEditor(group: $groups[index], endpoints: endpoints, tab: $tab)
                    .id(groups[index].id).disabled(saving)
            }
        }.font(.system(size: 13)).controlSize(.regular)
    }
    private var heading: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("模型组").font(.system(size: 18, weight: .semibold))
            Text("按组优先级 → 入口优先级调度；数字越小越优先，同级按排列顺序。")
                .font(.system(size: 12)).foregroundStyle(palette.textSecondary)
        }
    }
    private var actions: some View {
        HStack(spacing: 8) {
            Button("新建模型组", systemImage: "plus") {
                let group = ModelGroup(); groups.append(group); selectedID = group.id; tab = "models"
            }
            if dirty { Button("撤销更改", action: revert) }
            Button(saving ? "保存中…" : "保存更改", action: save)
                .buttonStyle(.borderedProminent).disabled(!canSave)
        }.disabled(saving)
    }
}

private struct ModelGroupSettings: View {
    @Binding var group: ModelGroup
    let canMoveUp: Bool; let canMoveDown: Bool
    let moveUp: () -> Void; let moveDown: () -> Void; let delete: () -> Void

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 16) { name.frame(maxWidth: 360); options.fixedSize(horizontal: true, vertical: false); Spacer(minLength: 0) }
            VStack(alignment: .leading, spacing: 8) { name; options }
        }
    }
    private var name: some View {
        HStack(spacing: 8) {
            Text("组名称").foregroundStyle(.secondary).fixedSize()
            TextField("组名称", text: $group.name).textFieldStyle(.roundedBorder).frame(minWidth: 140)
        }
    }
    private var options: some View {
        HStack(spacing: 12) {
            HStack(spacing: 8) {
                Text("优先级").foregroundStyle(.secondary).fixedSize()
                TextField("组优先级", value: $group.priority, format: .number)
                    .textFieldStyle(.roundedBorder).frame(width: 64).help("数字越小越优先；同级按顶部排列顺序。")
            }
            Toggle("启用该组", isOn: $group.enabled).toggleStyle(.checkbox).fixedSize()
            Spacer(minLength: 0)
            Menu("组操作") {
                Button("前移", systemImage: "arrow.left", action: moveUp).disabled(!canMoveUp)
                Button("后移", systemImage: "arrow.right", action: moveDown).disabled(!canMoveDown)
                Divider()
                Button("删除模型组", role: .destructive, action: delete)
            }.fixedSize()
        }
    }
}

struct ModelGroupEditor: View {
    @Binding var group: ModelGroup
    let endpoints: [Endpoint]
    @State private var endpointToAdd = ""
    @Binding var tab: String
    @Environment(\.sumpterPalette) private var palette
    private var categories: [ModelGroupCatalogCategory] {
        ModelGroupCatalog.categories(endpoints: endpoints)
    }
    private var unlistedModels: [String] {
        let listed = Set(categories.flatMap(\.models))
        return group.models.filter { !listed.contains($0) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Picker("编辑内容", selection: $tab) {
                Text("模型选择 · \(group.models.count)").tag("models")
                Text("入口绑定 · \(group.bindings.count)").tag("bindings")
            }.pickerStyle(.segmented).labelsHidden().frame(maxWidth: 340)
            Divider()
            if tab == "models" { modelsPanel } else { bindingsPanel }
        }
        .padding(12).background(palette.surface, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(palette.borderSubtle))
    }

    private var modelsPanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("已选 \(group.models.count - unlistedModels.count) 个模型").fontWeight(.medium)
                Spacer()
                Button("全选已添加") { group.models = Array(Set(group.models + categories.flatMap(\.models))).sorted() }
                Button("清空") { group.models.removeAll(); prune() }
            }
            Text("仅汇总入口库已添加的模型；点击模型整行即可勾选。")
                .font(.system(size: 12)).foregroundStyle(palette.textSecondary)
            ModelCategoryPicker(categories: categories, selected: Binding(
                get: { Set(group.models) }, set: { group.models = Array($0).sorted(); prune() }
            ))
            if !unlistedModels.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Label("\(unlistedModels.count) 项历史选择未在入口库添加，已从列表隐藏。", systemImage: "info.circle")
                        .font(.caption).foregroundStyle(.secondary)
                    Button("清理历史选择") {
                        let listed = Set(categories.flatMap(\.models))
                        group.models.removeAll { !listed.contains($0) }; prune()
                    }
                }
            }
            if uncovered {
                Label("部分模型尚未配置启用的承接入口；请求会继续检查其他组。", systemImage: "info.circle")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }
    private var bindingsPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            if group.id == "default" {
                Label("默认组已绑定入口的顺序与优先级跟随入口库，请到入口库调整。新入口不会自动加入任何模型组，请在此手动添加并选择承接范围。", systemImage: "arrow.triangle.2.circlepath")
                    .font(.system(size: 12)).foregroundStyle(palette.textSecondary)
            }
            HStack {
                Picker("从入口库添加", selection: $endpointToAdd) {
                    Text("选择入口…").tag("")
                    ForEach(endpoints.filter { e in !group.bindings.contains { $0.endpointID == e.id } }) { e in
                        Text(e.name + (e.enabled ? "" : "（入口已停用）")).tag(e.id)
                    }
                }.frame(maxWidth: 320)
                Button("添加", systemImage: "plus") {
                    group.bindings.append(ModelGroupBinding(endpointID: endpointToAdd)); endpointToAdd = ""
                }.disabled(endpointToAdd.isEmpty)
                Spacer()
            }
            if group.bindings.isEmpty { Text("添加入口后，可从该入口支持的模型中选择承接范围。").foregroundStyle(.secondary) }
            LazyVStack(alignment: .leading, spacing: 6) {
                ForEach(Array(group.bindings.enumerated()), id: \.element.endpointID) { index, binding in
                    ModelGroupBindingEditor(binding: $group.bindings[index], models: group.models,
                        endpoint: endpoints.first { $0.id == binding.endpointID },
                        canMoveUp: index > 0, canMoveDown: index + 1 < group.bindings.count,
                        up: { group.bindings.swapAt(index, index - 1) }, down: { group.bindings.swapAt(index, index + 1) },
                        remove: { group.bindings.remove(at: index) },
                        followsLibrary: group.id == "default")
                }
            }
        }
    }
    private var uncovered: Bool {
        group.models.contains { model in !group.bindings.contains { binding in
            binding.enabled && endpoints.contains {
                $0.id == binding.endpointID && $0.enabled && $0.preferredMapping(for: model) != nil
            }
                && (binding.models.map { $0.contains { ModelName.matches(model, pattern: $0) } } ?? true)
        } }
    }
    private func prune() {
        var config = AppConfig(endpoints: endpoints, modelGroups: [group])
        config.pruneDanglingEndpointReferences(); group = config.modelGroups?.first ?? group
    }
}
