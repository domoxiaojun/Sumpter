import SwiftUI
import SumpterCore

/// Family navigation and selection are separate full-size controls. A model's
/// entire tile is a native Toggle, including its trailing whitespace.
struct ModelCategoryPicker: View {
    let categories: [ModelGroupCatalogCategory]
    @Binding var selected: Set<String>
    @State private var query = ""
    @State private var activeID: String?
    @Environment(\.sumpterPalette) private var palette

    private var searching: Bool { !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private var visible: [ModelGroupCatalogCategory] {
        if !searching { return categories }
        return categories.compactMap { category in
            let models = category.models.filter { $0.localizedCaseInsensitiveContains(query.trimmingCharacters(in: .whitespacesAndNewlines)) }
            return models.isEmpty ? nil : ModelGroupCatalogCategory(id: category.id, title: category.title, models: models)
        }
    }
    private var displayed: [ModelGroupCatalogCategory] {
        if searching { return visible }
        return (categories.first { $0.id == activeID } ?? categories.first).map { [$0] } ?? []
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass").foregroundStyle(palette.textSecondary)
                TextField("搜索模型", text: $query).textFieldStyle(.plain)
                if searching {
                    Button { query = "" } label: { Image(systemName: "xmark.circle.fill") }
                        .buttonStyle(.plain).help("清除搜索").accessibilityLabel("清除搜索")
                }
            }
            .padding(.horizontal, 10).frame(maxWidth: 380, minHeight: 32)
            .background(palette.inset, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(palette.borderMedium))

            if !searching {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 130, maximum: 180), spacing: 6)], alignment: .leading, spacing: 6) {
                    ForEach(categories) { category in
                        let active = displayed.first?.id == category.id
                        Button { activeID = category.id } label: {
                            HStack(spacing: 8) {
                                Text(category.title).font(.system(size: 13, weight: .medium))
                                Spacer(minLength: 4)
                                Text("\(category.models.filter { selected.contains($0) }.count)/\(category.models.count)")
                                    .font(.caption.monospacedDigit()).foregroundStyle(palette.textSecondary)
                            }
                            .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 32)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(ModelGroupTileStyle(selected: active))
                        .accessibilityLabel("\(category.title) 系列")
                        .accessibilityValue(active ? "当前系列" : "切换系列")
                    }
                }
            }

            ForEach(displayed) { category in
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("\(category.title) 系列").font(.system(size: 13, weight: .medium))
                        Text("\(category.models.count) 个模型").font(.caption).foregroundStyle(palette.textSecondary)
                        Spacer()
                        Button("全选") { selected.formUnion(category.models) }
                        Button("清空") { selected.subtract(category.models) }
                    }
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 230), spacing: 6)], spacing: 4) {
                        ForEach(category.models, id: \.self) { name in
                            Toggle(name, isOn: Binding(
                                get: { selected.contains(name) },
                                set: { if $0 { selected.insert(name) } else { selected.remove(name) } }
                            ))
                            .toggleStyle(ModelGroupTileToggleStyle())
                        }
                    }
                }
                .padding(.top, 2)
            }
            if displayed.isEmpty {
                Text(searching ? "没有匹配的模型，试试其他关键词。" : "暂无可选模型，请先在入口库添加模型，并确认当前组的模型范围。")
                    .font(.system(size: 13)).foregroundStyle(palette.textSecondary).padding(.vertical, 8)
            }
        }
    }
}

struct ModelGroupTileToggleStyle: ToggleStyle {
    @Environment(\.sumpterPalette) private var palette
    func makeBody(configuration: Configuration) -> some View {
        Button { configuration.isOn.toggle() } label: {
            HStack(alignment: .center, spacing: 10) {
                Image(systemName: configuration.isOn ? "checkmark.square.fill" : "square")
                    .font(.system(size: 16)).foregroundStyle(configuration.isOn ? palette.brand : palette.textMuted)
                configuration.label.font(.system(size: 13)).multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .frame(maxWidth: .infinity, minHeight: 36, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(ModelGroupTileStyle())
        .accessibilityValue(configuration.isOn ? "已选择" : "未选择")
    }
}

struct ModelGroupTileStyle: ButtonStyle {
    var selected = false
    var borderless = false
    @Environment(\.sumpterPalette) private var palette
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        Tile(configuration: configuration, selected: selected, borderless: borderless, palette: palette)
            .opacity(enabled ? 1 : 0.45)
    }

    private struct Tile: View {
        let configuration: Configuration
        let selected: Bool
        let borderless: Bool
        let palette: SumpterTheme.Palette
        @State private var hovering = false
        var body: some View {
            configuration.label
                .foregroundStyle(palette.textPrimary)
                .background(borderless ? .clear : selected ? palette.active : palette.surface, in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).fill(configuration.isPressed || hovering ? palette.hover : .clear))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(borderless ? .clear : selected ? palette.brand : palette.borderMedium, lineWidth: selected ? 1.5 : 1))
                .onHover { hovering = $0 }
        }
    }
}
