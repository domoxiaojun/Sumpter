import SumpterCore
import SwiftUI

struct RoutingPane: View {
    @ObservedObject var model: AppModel

    var body: some View {
        SettingsPage(title: SettingsSection.routing.title, subtitle: SettingsSection.routing.subtitle) {
            poolPatternsPanel
            FeatureRulesPanel(model: model)
        }
    }

    private var poolPatternsPanel: some View {
        SectionPanel(title: "Claude Code 路由摘要", hint: "按 Provider 候选序列与入口显式映射命中；三类 Claude Code 子请求可独立覆盖目标与 effort。") {
            PoolSummaryList(config: model.config)
        }
    }
}

private struct FeatureRulesPanel: View {
    @ObservedObject var model: AppModel
    @State private var selection = Set<String>()
    @State private var editingRule: FeatureRule?

    var body: some View {
        SectionPanel(title: "预设分流", hint: "识别条件由Sumpter维护，可调整启用状态、目标模型与 effort 覆盖。") {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Button {
                        editingRule = selectedRule
                    } label: {
                        Label("编辑目标", systemImage: "square.and.pencil")
                    }
                    .disabled(selectedRule == nil)
                    Spacer()
                }
                if rows.isEmpty {
                    EmptyStateView(title: "暂无分流规则", systemImage: "arrow.triangle.branch")
                } else {
                    GeometryReader { proxy in
                        if proxy.size.width < 680 {
                            compactRuleList
                        } else {
                            rulesTable
                        }
                    }
                    .frame(height: adaptiveTableHeight(
                        rows: rows.count,
                        min: rows.count >= 3 ? 136 : 96,
                        max: 300,
                        rowHeight: 52
                    ))
                }
            }
        }
        .sheet(item: $editingRule) { rule in
            FeatureRuleEditorSheet(model: model, rule: rule) { editingRule = nil }
        }
    }

    private var rows: [FeatureRuleDisplayRow] {
        model.config.featureRules.map { FeatureRuleDisplayRow(rule: $0, endpoints: model.config.endpoints) }
    }

    private var rulesTable: some View {
        Table(rows, selection: $selection) {
            TableColumn("状态") { row in
                StatusBadge(
                    text: row.enabled ? "启用" : "停用",
                    systemImage: row.enabled ? "checkmark.circle.fill" : "pause.circle",
                    color: row.enabled ? .green : .secondary
                )
            }
            .width(min: 70, ideal: 78, max: 90)
            TableColumn("名称") { row in
                HStack(spacing: 6) {
                    if row.isBuiltIn {
                        Image(systemName: "lock.fill")
                            .foregroundStyle(.secondary)
                    }
                    Text(row.name)
                        .lineLimit(1)
                        .help(row.name)
                }
            }
            .width(min: 130, ideal: 180)
            TableColumn("识别条件") { row in
                Text(row.matchSummary.isEmpty ? "-" : row.matchSummary)
                    .lineLimit(2)
                    .truncationMode(.tail)
                    .help(row.matchSummary)
            }
            .width(min: 180, ideal: 300)
            TableColumn("目标") { row in
                Text(row.targetSummary)
                    .lineLimit(2)
                    .truncationMode(.tail)
                    .help(row.targetSummary)
            }
            .width(min: 170, ideal: 240)
        }
        .kekulvTableSurface()
        .contextMenu(forSelectionType: String.self) { ids in
            if ids.count == 1, let rule = rule(id: ids.first) {
                Button("编辑目标") { editingRule = rule }
                Button(rule.enabled ? "停用" : "启用") { setEnabled(rule, !rule.enabled) }
            }
        } primaryAction: { ids in
            if ids.count == 1, let rule = rule(id: ids.first) { editingRule = rule }
        }
    }

    private var compactRuleList: some View {
        ScrollView(.vertical, showsIndicators: true) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(rows) { row in
                    Button {
                        selection = [row.id]
                        editingRule = rowRule(row)
                    } label: {
                        VStack(alignment: .leading, spacing: 6) {
                            HStack(spacing: 8) {
                                StatusBadge(
                                    text: row.enabled ? "启用" : "停用",
                                    systemImage: row.enabled ? "checkmark.circle.fill" : "pause.circle",
                                    color: row.enabled ? .green : .secondary
                                )
                                if row.isBuiltIn {
                                    Image(systemName: "lock.fill")
                                        .foregroundStyle(.secondary)
                                        .accessibilityLabel("内置规则")
                                }
                                Text(row.name)
                                    .font(.callout.weight(.semibold))
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                                Image(systemName: "chevron.right")
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(.tertiary)
                                    .accessibilityHidden(true)
                            }
                            Text(row.matchSummary.isEmpty ? "未设置识别条件" : row.matchSummary)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(2)
                                .fixedSize(horizontal: false, vertical: true)
                            Text(row.targetSummary)
                                .font(.caption)
                                .foregroundStyle(.primary)
                                .lineLimit(2)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 9)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(selection.contains(row.id) ? Color.accentColor.opacity(0.08) : .clear)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .contextMenu {
                        Button("编辑目标") { editingRule = rowRule(row) }
                        Button(row.enabled ? "停用" : "启用") {
                            if let rule = rowRule(row) { setEnabled(rule, !rule.enabled) }
                        }
                    }
                    if row.id != rows.last?.id {
                        Divider().opacity(0.35)
                    }
                }
            }
        }
        .background(Color(nsColor: .textBackgroundColor).opacity(0.25))
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private func rowRule(_ row: FeatureRuleDisplayRow) -> FeatureRule? {
        rule(id: row.id)
    }

    private var selectedRule: FeatureRule? {
        rule(id: selection.count == 1 ? selection.first : nil)
    }

    private func rule(id: String?) -> FeatureRule? {
        guard let id else { return nil }
        return model.config.featureRules.first { $0.id == id }
    }

    private func setEnabled(_ rule: FeatureRule, _ enabled: Bool) {
        Task {
            do {
                try await model.updateFeatureRuleAndSave(
                    id: rule.id,
                    enabled: enabled,
                    model: rule.target.model,
                    effortOverride: rule.target.effortOverride,
                    protocolOverride: rule.target.protocolOverride,
                    endpointID: rule.target.endpointID,
                    toolTypePrefix: rule.match.toolTypePrefix ?? "",
                    systemContains: rule.match.systemContains ?? "",
                    messagesContain: rule.match.messagesContain ?? "",
                    modelEquals: rule.match.modelEquals ?? ""
                )
            } catch {
                model.lastError = "\(error)"
            }
        }
    }
}
