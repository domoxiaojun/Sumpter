import AppKit
import Foundation
import SumpterCore
import SwiftUI
import UniformTypeIdentifiers

private enum ProviderEditorMode: Identifiable {
    case provider(providerName: String, row: EndpointDisplayRow?)

    var id: String {
        switch self {
        case .provider(let providerName, let row):
            "provider-\(providerName)-\(row?.id ?? "add")"
        }
    }
}

private enum ProviderDeleteRequest: Identifiable {
    case providers(ids: Set<String>, names: [String])
    case mappings(endpointID: String, ids: Set<String>, names: [String])

    var id: String {
        switch self {
        case .providers(let ids, _):
            "providers-\(ids.sorted().joined(separator: ","))"
        case .mappings(let endpointID, let ids, _):
            "mappings-\(endpointID)-\(ids.sorted().joined(separator: ","))"
        }
    }

    /// 单删标题直接带名称;多删标题只带数量,名称在 message 里列全。
    var title: String {
        switch self {
        case .providers(let ids, let names):
            ids.count == 1 ? "删除入口「\(names.first ?? ids.first ?? "")」？" : "删除 \(ids.count) 个入口？"
        case .mappings(_, let ids, let names):
            ids.count == 1 ? "删除模型映射「\(names.first ?? "")」？" : "删除 \(ids.count) 个模型映射？"
        }
    }

    var detailText: String {
        let names: [String] = switch self {
        case .providers(_, let names): names
        case .mappings(_, _, let names): names
        }
        guard names.count > 1 else { return "" }
        return names.joined(separator: "、")
    }
}

private enum MappingSheetMode: Identifiable {
    case add(endpointID: String)
    case edit(MappingDisplayRow)
    case addFromCatalog(endpointID: String)

    var id: String {
        switch self {
        case .add(let endpointID):
            "add-\(endpointID)"
        case .edit(let row):
            "edit-\(row.endpointID)-\(row.id)"
        case .addFromCatalog(let endpointID):
            "catalog-\(endpointID)"
        }
    }
}

struct ProvidersPane: View {
    @ObservedObject var model: AppModel
    @State private var providerSelection = Set<String>()
    @State private var mappingSelection = Set<String>()
    @State private var togglingEndpointIDs = Set<String>()
    @State private var editorMode: ProviderEditorMode?
    @State private var retrySheetPresented = false
    @State private var mappingSheet: MappingSheetMode?
    @State private var deleteRequest: ProviderDeleteRequest?
    /// 排序保存必须等 AppKit 结束当前拖拽会话后再发布列表数据；同时
    /// 取消尚未执行的旧操作，避免快速连续拖拽时多个配置快照互相覆盖。
    @State private var pendingReorderTask: Task<Void, Never>?

    var body: some View {
        SettingsPage(title: SettingsSection.providers.title, subtitle: SettingsSection.providers.subtitle) {
            let rows = model.config.endpointRows
            // 顶部摘要区保持双栏：左侧先回答“有哪些入口、当前状态如何”，
            // 右侧集中放转发与重试参数；窗口收窄后再纵向排列。
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .top, spacing: SumpterTheme.Layout.panelSpacing) {
                    providerOverviewPanel(rows: rows)
                        .frame(
                            minWidth: 360,
                            idealWidth: 420,
                            maxWidth: 460,
                            maxHeight: .infinity,
                            alignment: .topLeading
                        )
                    retryPolicyPanel
                        .frame(
                            minWidth: 480,
                            maxWidth: .infinity,
                            maxHeight: .infinity,
                            alignment: .topLeading
                        )
                }
                VStack(alignment: .leading, spacing: SumpterTheme.Layout.panelSpacing) {
                    providerOverviewPanel(rows: rows)
                    retryPolicyPanel
                }
            }
            // 入口详情平铺在入口列表下方，模型映射再紧随其后，保持一条清晰的
            // “先选入口 → 看入口配置 → 看该入口映射”阅读路径。
            VStack(alignment: .leading, spacing: SumpterTheme.Layout.panelSpacing) {
                providerAccountsPanel(
                    title: "上游通道入口列表（\(rows.count)）",
                    hint: "调度优先按 Priority 数值（小者优先），同级按列表顺序。点击行查看详细映射；拖动整行可调整入口顺序，拖动表头分隔线可调整列宽。",
                    rows: rows,
                    selection: $providerSelection
                )
                endpointDetailPanel(title: "入口详情", rows: rows, selection: providerSelection)
                providerMappingPanel(
                    selected: selectedRow(rows, selection: providerSelection),
                    mappingRows: providerMappingRows(endpointID: selectedRow(rows, selection: providerSelection)?.id),
                    mappingSelection: $mappingSelection
                )
            }
        }
        .toolbar {
            ToolbarItem {
                Button {
                    editorMode = .provider(providerName: "Provider", row: nil)
                } label: {
                    Label("添加入口", systemImage: "plus")
                }
                .help("添加 Provider 入口")
            }
        }
        .onAppear(perform: sanitizeSelections)
        .onChange(of: model.config) { _, _ in sanitizeSelections() }
        .onDisappear {
            pendingReorderTask?.cancel()
            pendingReorderTask = nil
        }
        .sheet(item: $editorMode) { mode in
            switch mode {
            case .provider(let providerName, let row):
                ProviderAccountEditorSheet(model: model, providerName: providerName, row: row) {
                    editorMode = nil
                }
            }
        }
        .sheet(isPresented: $retrySheetPresented) {
            RetryPolicySheet(model: model) {
                retrySheetPresented = false
            }
        }
        .sheet(item: $mappingSheet) { sheet in
            switch sheet {
            case .add(let endpointID):
                MappingEditorSheet(model: model, endpointID: endpointID, mapping: nil) {
                    mappingSheet = nil
                }
            case .edit(let row):
                MappingEditorSheet(model: model, endpointID: row.endpointID, mapping: row) {
                    mappingSheet = nil
                }
            case .addFromCatalog(let endpointID):
                CatalogMappingSheet(model: model, endpointID: endpointID) {
                    mappingSheet = nil
                }
            }
        }
        .confirmationDialog(deleteRequest?.title ?? "删除？", isPresented: Binding(
            get: { deleteRequest != nil },
            set: { if !$0 { deleteRequest = nil } }
        )) {
            Button("删除", role: .destructive) {
                performDelete()
            }
            Button("取消", role: .cancel) {
                deleteRequest = nil
            }
        } message: {
            // 多删时把名称列全,单删名称已在标题里。
            if let detail = deleteRequest?.detailText, !detail.isEmpty {
                Text(detail)
            }
        }
    }

    private func providerOverviewPanel(rows: [EndpointDisplayRow]) -> some View {
        let enabledCount = rows.filter(\.enabled).count
        let mappingCount = rows.reduce(0) { $0 + $1.mappingCount }
        // 同一模型往往会被多个入口同时返回；概览回答“共有多少种模型”，
        // 因此跨入口也只计一次，入口行仍各自显示自己的目录数量。
        let catalogModelCount = Set(
            rows.flatMap { $0.catalog.uniqueModels.map(ModelName.clean) }
                .filter { !$0.isEmpty }
        ).count
        return ProviderSummaryPanel(
            title: "入口概览",
            hint: "当前 Provider 入口数量、启用状态、模型映射和目录获取情况。",
            action: { EmptyView() },
            content: {
            overviewMetrics(
                columns: [
                    GridItem(.flexible(), spacing: 10),
                    GridItem(.flexible(), spacing: 10),
                    GridItem(.flexible(), spacing: 10)
                ],
                rows: rows,
                enabledCount: enabledCount,
                mappingCount: mappingCount,
                catalogModelCount: catalogModelCount
            )
            }
        )
    }

    private func overviewMetrics(
        columns: [GridItem],
        rows: [EndpointDisplayRow],
        enabledCount: Int,
        mappingCount: Int,
        catalogModelCount: Int
    ) -> some View {
        LazyVGrid(columns: columns, spacing: 10) {
            ProviderPolicySummaryItem(title: "入口总数", value: String(rows.count))
            ProviderPolicySummaryItem(title: "已启用", value: String(enabledCount))
            ProviderPolicySummaryItem(title: "未启用", value: String(max(0, rows.count - enabledCount)))
            ProviderPolicySummaryItem(title: "模型映射", value: String(mappingCount) + " 条")
            ProviderPolicySummaryItem(
                title: "目录模型（去重）",
                value: catalogModelCount == 0 ? "尚未获取" : String(catalogModelCount) + " 个"
            )
        }
    }

    private var retryPolicyPanel: some View {
        ProviderSummaryPanel(
            title: "参数设置",
            hint: "全局作用于所有 Provider 入口；故障时先重试当前粘性分组，再按优先级切换下一候选。",
            action: {
                Button {
                    retrySheetPresented = true
                } label: {
                    Label("编辑参数", systemImage: "slider.horizontal.3")
                }
                .controlSize(.small)
                .fixedSize()
            },
            content: {
                let retry = model.config.retry
                policyMetrics(
                    columns: [
                        GridItem(.flexible(), spacing: 10),
                        GridItem(.flexible(), spacing: 10),
                        GridItem(.flexible(), spacing: 10)
                    ],
                    retry: retry
                )
            }
        )
    }

    private func policyMetrics(
        columns: [GridItem],
        retry: RetryPolicy
    ) -> some View {
        LazyVGrid(columns: columns, spacing: 10) {
            ProviderPolicySummaryItem(title: "单次超时", value: timeoutSummary(retry.responseTimeoutSeconds))
            ProviderPolicySummaryItem(title: "吐字超时", value: timeoutSummary(retry.streamIdleTimeoutSeconds))
            ProviderPolicySummaryItem(
                title: "故障重试轮数",
                value: retry.maxDeferredRounds == 0 ? "不限" : "\(retry.maxDeferredRounds)"
            )
            ProviderPolicySummaryItem(
                title: "跨轮时限",
                value: retry.maxRetryDurationSeconds == 0 ? "不限" : "\(retry.maxRetryDurationSeconds)s"
            )
            ProviderPolicySummaryItem(title: "粘性重试", value: "\(retry.sessionStickyRetries) 次")
            ProviderPolicySummaryItem(title: "IP 并发", value: "\(retry.pinnedIPConcurrency)")
        }
    }

    private func providerAccountsPanel(
        title: String,
        hint: String,
        rows: [EndpointDisplayRow],
        selection: Binding<Set<String>>
    ) -> some View {
        return SectionPanel(title: title, hint: hint) {
            ProviderAccountsTable(
                rows: rows,
                selection: selection,
                showsQuickToggle: true,
                togglingEndpointIDs: togglingEndpointIDs,
                fetchingModelEndpointIDs: model.fetchingModelEndpointIDs,
                onEdit: { editorMode = .provider(providerName: $0.name, row: $0) },
                onSetEnabled: setEnabled,
                onFetchModels: fetchModels,
                onCopyBaseURL: copyBaseURL,
                onDeleteIDs: { deleteRequest = providerDeleteRequest(rows: rows, ids: $0) },
                onMove: { row, direction in
                    moveSelection(ids: [row.id], direction: direction)
                },
                onReorder: { draggedID, targetID, placeAfter in
                    reorderProvider(draggedID: draggedID, targetID: targetID, placeAfter: placeAfter, rows: rows)
                }
            )
        }
    }

    private func providerMappingPanel(
        selected: EndpointDisplayRow?,
        mappingRows: [MappingDisplayRow],
        mappingSelection: Binding<Set<String>>
    ) -> some View {
        SectionPanel(
            title: selected == nil ? "模型映射" : "\(selected?.name ?? "") 的模型映射",
            hint: "客户端模型 -> 上游模型 / Thinking / 1M / 首响应超时；每个入口必须显式声明需要承接的模型。"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 8) {
                        mappingActionButtons(
                            selected: selected,
                            mappingRows: mappingRows,
                            mappingSelection: mappingSelection.wrappedValue
                        )
                        Spacer(minLength: 8)
                    }
                    VStack(alignment: .leading, spacing: 8) {
                        mappingActionButtons(
                            selected: selected,
                            mappingRows: mappingRows,
                            mappingSelection: mappingSelection.wrappedValue
                        )
                    }
                }
                MappingTable(
                    rows: mappingRows,
                    selection: mappingSelection,
                    onEdit: { mappingSheet = .edit($0) },
                    onDeleteIDs: { ids in
                        if let selected {
                            deleteRequest = mappingDeleteRequest(endpointID: selected.id, rows: mappingRows, ids: ids)
                        }
                    }
                )
            }
        }
    }

    @ViewBuilder
    private func mappingActionButtons(
        selected: EndpointDisplayRow?,
        mappingRows: [MappingDisplayRow],
        mappingSelection: Set<String>
    ) -> some View {
        Button {
            if let selected {
                mappingSheet = .add(endpointID: selected.id)
            }
        } label: {
            Label("添加", systemImage: "plus")
        }
        .disabled(selected == nil)
        Button {
            if let row = selectedRow(mappingRows, selection: mappingSelection) {
                mappingSheet = .edit(row)
            }
        } label: {
            Label("编辑", systemImage: "square.and.pencil")
        }
        .disabled(selectedRow(mappingRows, selection: mappingSelection) == nil)
        Button(role: .destructive) {
            if let selected, !mappingSelection.isEmpty {
                deleteRequest = mappingDeleteRequest(
                    endpointID: selected.id,
                    rows: mappingRows,
                    ids: mappingSelection
                )
            }
        } label: {
            Label("删除", systemImage: "trash")
        }
        .disabled(selected == nil || mappingSelection.isEmpty)
        if let selected {
            let unmapped = model.unmappedCatalogModels(endpointID: selected.id)
            Button {
                mappingSheet = .addFromCatalog(endpointID: selected.id)
            } label: {
                Label("从已知模型添加\(unmapped.isEmpty ? "" : " (\(unmapped.count))")", systemImage: "square.grid.2x2")
            }
            .disabled(unmapped.isEmpty)
        }
    }

    @ViewBuilder
    private func endpointDetailPanel(
        title: String,
        rows: [EndpointDisplayRow],
        selection: Set<String>
    ) -> some View {
        let row = selectedRow(rows, selection: selection)
        SectionPanel(title: title) {
            if let row {
                // 详情是正文信息，不再给每个字段套一层卡片。使用自适应网格：
                // 宽窗口一行 3 项，中等宽度一行 2 项，窄窗口自然降为单列。
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 300), spacing: 28)],
                    alignment: .leading,
                    // 详情是连续正文，行间只留一个小的阅读节奏；避免
                    // 每一行看起来像独立卡片，同时让四行字段在默认窗口内完整可见。
                    spacing: 6
                ) {
                    EndpointDetailField(title: "ID", value: row.id, copyable: true)
                    EndpointDetailField(title: "名称", value: row.name)
                    EndpointDetailField(title: "优先级", value: row.priorityText)
                    EndpointDetailField(title: "入口协议", value: row.protocolDisplayName)
                    EndpointDetailField(title: "API 地址", value: row.baseURL, copyable: true)
                    EndpointDetailField(title: "API Key", value: row.keyStatusText)
                    EndpointDetailField(title: "出口模式", value: row.pinModeText)
                    EndpointDetailField(title: "Pinned IPs", value: row.pinnedIPs.isEmpty ? "-" : row.pinnedIPsText, copyable: !row.pinnedIPs.isEmpty)
                    EndpointDetailField(title: "粘性分组", value: row.stickyGroupText)
                    EndpointDetailField(title: "连接复用", value: row.keepAliveText)
                    EndpointDetailField(title: "已知模型", value: row.modelCatalogText)
                    EndpointDetailField(title: "模型状态", value: row.modelCatalogStatusText)
                }
            } else {
                EmptyStateView(title: "选择一个入口查看详情", systemImage: "sidebar.leading")
            }
        }
    }

    /// 模型目录刷新属于“模型映射”操作，不再散落在入口详情底部。
    private func fetchModels(for row: EndpointDisplayRow) {
        Task {
            do {
                try await model.fetchProviderModels(endpointID: row.id)
            } catch {
                await model.markFetchModelsFailure(endpointID: row.id, error: error)
                model.lastError = "\(error)"
            }
        }
    }

    private func toolbar(
        add: @escaping () -> Void,
        edit: @escaping () -> Void,
        delete: @escaping () -> Void,
        canEdit: Bool,
        canDelete: Bool
    ) -> some View {
        HStack {
            Button(action: add) {
                Label("添加", systemImage: "plus")
            }
            Button(action: edit) {
                Label("编辑", systemImage: "square.and.pencil")
            }
            .disabled(!canEdit)
            Button(role: .destructive, action: delete) {
                Label("删除", systemImage: "trash")
            }
            .disabled(!canDelete)
            Spacer()
        }
    }

    private func providerMappingRows(endpointID: String?) -> [MappingDisplayRow] {
        guard let endpointID, let endpoint = model.config.endpoint(id: endpointID) else { return [] }
        return endpoint.mappings.map { MappingDisplayRow(endpoint: endpoint, mapping: $0) }
    }

    private func selectedRow<Row: Identifiable>(_ rows: [Row], selection: Set<String>) -> Row? where Row.ID == String {
        TableSelection.singleSelected(rows, selection: selection)
    }

    private func providerDeleteRequest(rows: [EndpointDisplayRow], ids: Set<String>) -> ProviderDeleteRequest {
        .providers(ids: ids, names: rows.filter { ids.contains($0.id) }.map(\.name))
    }

    private func mappingDeleteRequest(endpointID: String, rows: [MappingDisplayRow], ids: Set<String>) -> ProviderDeleteRequest {
        .mappings(endpointID: endpointID, ids: ids, names: rows.filter { ids.contains($0.id) }.map(\.clientPattern))
    }

    private func moveSelection(ids: Set<String>, direction: Int) {
        Task {
            do {
                try await model.moveProviderAccounts(ids: ids, direction: direction)
            } catch {
                model.lastError = "\(error)"
            }
        }
    }

    private func reorderProvider(
        draggedID: String,
        targetID: String,
        placeAfter: Bool,
        rows: [EndpointDisplayRow]
    ) {
        let orderedIDs = rows.map(\.id)
        guard draggedID != targetID,
              let sourceIndex = orderedIDs.firstIndex(of: draggedID),
              let targetIndex = orderedIDs.firstIndex(of: targetID) else {
            return
        }

        // Convert the visible before/after target into an index after the
        // dragged item is removed. This keeps downward moves from ending one
        // row too far below the drop target.
        var insertionIndex = targetIndex + (placeAfter ? 1 : 0)
        if sourceIndex < insertionIndex { insertionIndex -= 1 }
        // `dropDestination` runs inside AppKit's drag-session callback. Do not
        // publish `config` from that callback: the list may be invalidated while
        // the drag session is still closing, which presents as a frozen window.
        // A short, cancellable suspension moves the write to
        // the next run-loop turn and coalesces rapid successive drops.
        pendingReorderTask?.cancel()
        pendingReorderTask = Task { @MainActor in
            do {
                try await Task.sleep(nanoseconds: 120_000_000)
            } catch {
                return
            }
            guard !Task.isCancelled else { return }
            do {
                try await model.reorderProviderAccount(id: draggedID, toIndex: insertionIndex)
            } catch {
                model.lastError = "入口顺序保存失败：\(error)"
            }
        }
    }

    /// 行内/右键快捷启停共用事务式保存，防止连点；失败由模型回滚到原状态。
    private func setEnabled(_ row: EndpointDisplayRow, enabled: Bool) {
        guard row.enabled != enabled, !togglingEndpointIDs.contains(row.id) else { return }
        togglingEndpointIDs.insert(row.id)
        Task {
            defer { togglingEndpointIDs.remove(row.id) }
            do {
                try await model.setProviderEnabled(
                    id: row.id,
                    enabled: enabled
                )
            } catch {
                model.lastError = "\(error)"
            }
        }
    }

    private func copyBaseURL(_ row: EndpointDisplayRow) {
        model.flash(PasteboardCopy.write(row.baseURL) ? "地址已复制" : "复制失败")
    }

    private func performDelete() {
        guard let deleteRequest else { return }
        Task {
            do {
                switch deleteRequest {
                case .providers(let ids, _):
                    try await model.deleteProviderAccounts(ids: ids)
                    providerSelection.subtract(ids)
                case .mappings(let endpointID, let ids, _):
                    try await model.deleteProviderMappings(endpointID: endpointID, mappingIDs: ids)
                    mappingSelection.subtract(ids)
                }
            } catch {
                model.lastError = "\(error)"
            }
            self.deleteRequest = nil
            sanitizeSelections()
        }
    }

    private func sanitizeSelections() {
        let endpoints = model.config.endpoints
        sanitize(&providerSelection, validIDs: Set(endpoints.map(\.id)), selectFirst: true)
        // 与映射面板的展示逻辑一致:恰好单选入口时才有映射表(Set.first 在多选时取值随机)。
        let single = providerSelection.count == 1 ? providerSelection.first : nil
        sanitize(
            &mappingSelection,
            validIDs: Set(providerMappingRows(endpointID: single).map(\.id)),
            selectFirst: false
        )
    }

    private func sanitize(_ selection: inout Set<String>, validIDs: Set<String>, selectFirst: Bool) {
        selection = TableSelection.sanitize(selection, validIDs: validIDs, selectFirst: selectFirst)
    }
}

/// Provider 顶部摘要需要把“编辑参数”放进标题区，避免按钮挤压指标列。
/// 这是 Provider 页专用外壳，其表面、间距与 SectionPanel 保持一致。
private struct ProviderSummaryPanel<Content: View, Action: View>: View {
    let title: String
    let hint: String
    @ViewBuilder var action: Action
    @ViewBuilder var content: Content

    @Environment(\.kekulvPalette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: SumpterTheme.Layout.panelSpacing) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.headline)
                    Text(hint)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                action
            }
            Divider()
            content
        }
        .padding(SumpterTheme.Layout.panelPadding)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(
            RoundedRectangle(cornerRadius: SumpterTheme.Layout.panelRadius, style: .continuous)
                .fill(palette.surface)
        )
        .overlay(
            RoundedRectangle(cornerRadius: SumpterTheme.Layout.panelRadius, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }
}

private struct ProviderPolicySummaryItem: View {
    let title: String
    let value: String
    @Environment(\.kekulvPalette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Text(value)
                .font(.callout.monospacedDigit().weight(.semibold))
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .frame(maxWidth: .infinity, minHeight: 52, alignment: .topLeading)
        .background(palette.inset, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }
}

/// 入口详情使用正文式字段，不再把每个字段包成独立卡片。
/// 外层网格负责在宽窗口排成 2~3 列，窄窗口再回落到单列；长地址和状态文本允许换行。
private struct EndpointDetailField: View {
    let title: String
    let value: String
    var copyable = false

    @Environment(\.kekulvPalette) private var palette
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text("\(title)：")
                .font(.caption)
                .foregroundStyle(palette.textMuted)
            if copyable {
                Text(value.isEmpty ? "-" : value)
                    .font(.callout)
                    .foregroundStyle(palette.textPrimary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                Text(value.isEmpty ? "-" : value)
                    .font(.callout)
                    .foregroundStyle(palette.textPrimary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        // 正常字号收紧行高；辅助功能字号保留更宽松的触读空间，避免
        // 动态字体放大后出现上下挤压或截断。
        .frame(
            maxWidth: .infinity,
            minHeight: dynamicTypeSize.isAccessibilitySize ? 38 : 28,
            alignment: .leading
        )
        .accessibilityElement(children: .combine)
    }
}


/// Provider 主表只保留完成日常决策所需的核心字段。地址、Key 尾号、出口
/// 与连接复用仍在下方“入口详情”中完整展示；这样宽度可以随窗口缩放，
/// 而不是让一组固定宽度的表格列决定整个页面的最小尺寸。
private enum ProviderTableColumn: String, CaseIterable {
    case status
    case name
    case priority
    case protocolName
    case mapping
    case models
    case actions

    /// 默认值按“入口名称和目录内容优先”分配。状态列不再预留
    /// “启用/停用”文本，操作列也只为真正需要的按钮留空间。
    var defaultWidth: CGFloat {
        switch self {
        case .status: 72
        case .name: 240
        case .priority: 52
        case .protocolName: 108
        case .mapping: 64
        case .models: 180
        case .actions: 260
        }
    }

    var minimumWidth: CGFloat {
        switch self {
        case .status: 60
        case .name: 150
        case .priority: 48
        case .protocolName: 92
        case .mapping: 60
        case .models: 150
        // Four compact icon buttons plus the labelled model action must remain
        // intact even at the smallest user-selected width.
        case .actions: 252
        }
    }

    var maximumWidth: CGFloat {
        switch self {
        case .status: 120
        case .name: 460
        case .priority: 110
        case .protocolName: 240
        case .mapping: 170
        case .models: 420
        case .actions: 440
        }
    }

    static let rowHeight: CGFloat = 64
    static let compactBreakpoint: CGFloat = 760
}

/// The Provider list is intentionally a custom row list (native `Table` makes
/// row drag and range selection compete with each other).  Keeping widths in
/// one value type gives the header and every row exactly the same geometry,
/// while allowing the user to tune the table for their own endpoint names.
private struct ProviderTableColumnWidths: Equatable, Codable {
    var status = ProviderTableColumn.status.defaultWidth
    var name = ProviderTableColumn.name.defaultWidth
    var priority = ProviderTableColumn.priority.defaultWidth
    var protocolName = ProviderTableColumn.protocolName.defaultWidth
    var mapping = ProviderTableColumn.mapping.defaultWidth
    var models = ProviderTableColumn.models.defaultWidth
    var actions = ProviderTableColumn.actions.defaultWidth

    static let storageKey = "kekulv.providers.table.columnWidths"

    var total: CGFloat {
        status + name + priority + protocolName + mapping + models + actions
    }

    subscript(column: ProviderTableColumn) -> CGFloat {
        get {
            switch column {
            case .status: status
            case .name: name
            case .priority: priority
            case .protocolName: protocolName
            case .mapping: mapping
            case .models: models
            case .actions: actions
            }
        }
        set {
            switch column {
            case .status: status = newValue
            case .name: name = newValue
            case .priority: priority = newValue
            case .protocolName: protocolName = newValue
            case .mapping: mapping = newValue
            case .models: models = newValue
            case .actions: actions = newValue
            }
        }
    }

    static func load() -> Self {
        guard let data = UserDefaults.standard.data(forKey: storageKey),
              let stored = try? JSONDecoder().decode(Self.self, from: data) else {
            return Self()
        }
        return stored.clamped()
    }

    func save() {
        guard let data = try? JSONEncoder().encode(clamped()) else { return }
        UserDefaults.standard.set(data, forKey: Self.storageKey)
    }

    func clamped() -> Self {
        var result = self
        for column in ProviderTableColumn.allCases {
            result[column] = min(
                column.maximumWidth,
                max(column.minimumWidth, result[column])
            )
        }
        return result
    }

}

/// A narrow, high-contrast resize affordance at the end of each header cell.
/// It intentionally lives outside the row drag surface, so changing a column
/// width can never reorder an endpoint by accident.
private struct ProviderResizableHeaderCell: View {
    let title: String
    @Binding var width: CGFloat
    let minimumWidth: CGFloat
    let maximumWidth: CGFloat
    let onResizeEnded: () -> Void

    @Environment(\.kekulvPalette) private var palette
    @State private var dragOrigin: CGFloat?
    @State private var isHoveringHandle = false

    var body: some View {
        ZStack(alignment: .trailing) {
            Text(title)
                .padding(.horizontal, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .allowsHitTesting(false)

            Rectangle()
                .fill(isHoveringHandle ? palette.brand.opacity(0.8) : palette.borderSubtle)
                .frame(width: 1, height: 18)
                .allowsHitTesting(false)

            Color.clear
                .frame(width: 12)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 2)
                        .onChanged { value in
                            if dragOrigin == nil {
                                dragOrigin = width
                            }
                            let origin = dragOrigin ?? width
                            width = min(
                                maximumWidth,
                                max(minimumWidth, origin + value.translation.width)
                            )
                        }
                        .onEnded { _ in
                            dragOrigin = nil
                            onResizeEnded()
                        }
                )
                .help("拖动调整「\(title)」列宽")
                .accessibilityLabel("调整「\(title)」列宽")
                .accessibilityHint("左右拖动调整宽度")
                .accessibilityValue(Text("当前宽度 \(Int(width)) 点"))
                .onHover { isHovering in
                    isHoveringHandle = isHovering
                    if isHovering {
                        NSCursor.resizeLeftRight.push()
                    } else {
                        NSCursor.pop()
                    }
                }
        }
        .frame(maxWidth: .infinity, minHeight: 32, maxHeight: 32, alignment: .leading)
        .onDisappear {
            if isHoveringHandle || dragOrigin != nil {
                NSCursor.pop()
            }
            dragOrigin = nil
            isHoveringHandle = false
        }
    }
}

private enum ProviderDropPlacement: Equatable {
    case before
    case after

    var label: String {
        switch self {
        case .before: "插入到此行之前"
        case .after: "插入到此行之后"
        }
    }

    var systemImage: String {
        switch self {
        case .before: "arrow.down.to.line"
        case .after: "arrow.up.to.line"
        }
    }
}

/// Provider 主表不能继续使用 `Table(selection:)`：原生 Table 会在拖拽源达到
/// 启动阈值前先处理行选择，并把鼠标经过的行解释成范围多选。这里使用可控的
/// 横向滚动行列表，让选择和拖拽成为两条独立的事件链。
private struct ProviderAccountsTable: View {
    let rows: [EndpointDisplayRow]
    @Binding var selection: Set<String>
    let showsQuickToggle: Bool
    let togglingEndpointIDs: Set<String>
    let fetchingModelEndpointIDs: Set<String>
    var onEdit: (EndpointDisplayRow) -> Void = { _ in }
    var onSetEnabled: (EndpointDisplayRow, Bool) -> Void = { _, _ in }
    var onFetchModels: (EndpointDisplayRow) -> Void = { _ in }
    var onCopyBaseURL: (EndpointDisplayRow) -> Void = { _ in }
    var onDeleteIDs: (Set<String>) -> Void = { _ in }
    var onMove: (EndpointDisplayRow, Int) -> Void = { _, _ in }
    var onReorder: (String, String, Bool) -> Void = { _, _, _ in }

    @Environment(\.kekulvPalette) private var palette
    @State private var selectionAnchor: String?
    @State private var hoveredRowID: String?
    @State private var draggedProviderID: String?
    @State private var columnWidths = ProviderTableColumnWidths.load()

    var body: some View {
        if rows.isEmpty {
            EmptyStateView(title: "暂无入口。", systemImage: "server.rack")
        } else {
            GeometryReader { proxy in
                // Keep the exact tuned widths while dragging.  The outer
                // scroll view absorbs any deficit on narrow windows; this
                // makes the handle track the pointer one-to-one.
                let visibleWidths = columnWidths
                Group {
                    if proxy.size.width < ProviderTableColumn.compactBreakpoint {
                        compactRows
                    } else {
                        ScrollView([.horizontal, .vertical], showsIndicators: true) {
                            VStack(alignment: .leading, spacing: 0) {
                                headerRow(widths: visibleWidths)
                                Divider()
                                    .overlay(palette.borderSubtle)
                                LazyVStack(alignment: .leading, spacing: 0) {
                                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                                        ProviderAccountListRow(
                                            row: row,
                                            widths: visibleWidths,
                                            isSelected: selection.contains(row.id),
                                            isHovered: hoveredRowID == row.id,
                                            showsQuickToggle: showsQuickToggle,
                                            isSaving: togglingEndpointIDs.contains(row.id),
                                            isFetchingModels: fetchingModelEndpointIDs.contains(row.id),
                                            draggedProviderID: $draggedProviderID,
                                            onSelect: { select(row.id) },
                                            onHover: hoverHandler(for: row.id),
                                            onEdit: { onEdit(row) },
                                            onSetEnabled: { onSetEnabled(row, $0) },
                                            onFetchModels: { onFetchModels(row) },
                                            onCopyBaseURL: { onCopyBaseURL(row) },
                                            onDelete: {
                                                onDeleteIDs(selection.contains(row.id) ? selection : [row.id])
                                            },
                                            canMoveUp: index > 0,
                                            canMoveDown: index < rows.count - 1,
                                            onMove: { direction in onMove(row, direction) },
                                            onReorder: { draggedID, placeAfter in
                                                onReorder(draggedID, row.id, placeAfter)
                                            }
                                        )
                                    }
                                }
                            }
                            .frame(minWidth: max(proxy.size.width, visibleWidths.total), alignment: .leading)
                        }
                    }
                }
                .background(palette.inset)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .stroke(palette.borderSubtle, lineWidth: 0.8)
                }
                .transaction { transaction in
                    transaction.animation = nil
                }
            }
            .frame(
                height: adaptiveTableHeight(
                    rows: rows.count,
                    min: 112,
                    max: 360,
                    rowHeight: ProviderTableColumn.rowHeight
                )
            )
            .onAppear {
                if selectionAnchor == nil {
                    selectionAnchor = selection.count == 1 ? selection.first : nil
                }
            }
            .onChange(of: selection) { _, updated in
                if updated.count == 1 {
                    selectionAnchor = updated.first
                }
            }
            .onDisappear {
                columnWidths.save()
            }
            .onDeleteCommand {
                if !selection.isEmpty { onDeleteIDs(selection) }
            }
        }
    }

    private func headerRow(widths: ProviderTableColumnWidths) -> some View {
        HStack(spacing: 0) {
            headerCell("状态", column: .status, widths: widths)
            headerCell("入口通道名称", column: .name, widths: widths)
            headerCell("优先级", column: .priority, widths: widths)
            headerCell("协议", column: .protocolName, widths: widths)
            headerCell("映射", column: .mapping, widths: widths)
            headerCell("模型目录", column: .models, widths: widths)
            headerCell("操作", column: .actions, widths: widths)
        }
        .frame(width: widths.total, height: 32, alignment: .leading)
        .font(.caption.weight(.semibold))
        .foregroundStyle(palette.textSecondary)
        .accessibilityElement(children: .contain)
        .contextMenu {
            Button("恢复默认列宽") {
                columnWidths = ProviderTableColumnWidths()
                columnWidths.save()
            }
        }
    }

    private func headerCell(
        _ title: String,
        column: ProviderTableColumn,
        widths: ProviderTableColumnWidths
    ) -> some View {
        ProviderResizableHeaderCell(
            title: title,
            width: Binding(
                get: { columnWidths[column] },
                set: { columnWidths[column] = $0 }
            ),
            minimumWidth: column.minimumWidth,
            maximumWidth: column.maximumWidth,
            onResizeEnded: { columnWidths.save() }
        )
        .frame(width: widths[column], alignment: .leading)
    }

    private func select(_ id: String) {
        let modifiers = NSEvent.modifierFlags
        if modifiers.contains(.shift),
           let anchor = selectionAnchor,
           let start = rows.firstIndex(where: { $0.id == anchor }),
           let end = rows.firstIndex(where: { $0.id == id }) {
            let rangeIDs = rows[min(start, end)...max(start, end)].map(\.id)
            if modifiers.contains(.command) {
                selection.formUnion(rangeIDs)
            } else {
                selection = Set(rangeIDs)
            }
        } else if modifiers.contains(.command) {
            if selection.contains(id) {
                selection.remove(id)
            } else {
                selection.insert(id)
            }
            selectionAnchor = id
        } else {
            selection = [id]
            selectionAnchor = id
        }
    }

    private func hoverHandler(for id: String) -> (Bool) -> Void {
        { hovered in
            if hovered {
                hoveredRowID = id
            } else if hoveredRowID == id {
                hoveredRowID = nil
            }
        }
    }

    /// Below the compact breakpoint, expose the three common actions (status,
    /// select/detail, edit) as a readable card.  The complete endpoint fields
    /// remain one tap away in the detail panel, and the existing keyboard
    /// up/down actions continue to provide a non-drag reorder path.
    private var compactRows: some View {
        ScrollView(.vertical, showsIndicators: true) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                    ProviderAccountCompactRow(
                        row: row,
                        isSelected: selection.contains(row.id),
                        showsQuickToggle: showsQuickToggle,
                        isSaving: togglingEndpointIDs.contains(row.id),
                        isFetchingModels: fetchingModelEndpointIDs.contains(row.id),
                        draggedProviderID: $draggedProviderID,
                        onSelect: { select(row.id) },
                        onEdit: { onEdit(row) },
                        onSetEnabled: { onSetEnabled(row, $0) },
                        onFetchModels: { onFetchModels(row) },
                        onCopyBaseURL: { onCopyBaseURL(row) },
                        onDelete: {
                            onDeleteIDs(selection.contains(row.id) ? selection : [row.id])
                        },
                        canMoveUp: index > 0,
                        canMoveDown: index < rows.count - 1,
                        onMove: { direction in onMove(row, direction) },
                        onReorder: { draggedID, placeAfter in
                            onReorder(draggedID, row.id, placeAfter)
                        }
                    )
                    if row.id != rows.last?.id {
                        Divider().opacity(0.35)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

private struct ProviderAccountCompactRow: View {
    let row: EndpointDisplayRow
    let isSelected: Bool
    let showsQuickToggle: Bool
    let isSaving: Bool
    let isFetchingModels: Bool
    @Binding var draggedProviderID: String?
    let onSelect: () -> Void
    let onEdit: () -> Void
    let onSetEnabled: (Bool) -> Void
    let onFetchModels: () -> Void
    let onCopyBaseURL: () -> Void
    let onDelete: () -> Void
    let canMoveUp: Bool
    let canMoveDown: Bool
    let onMove: (Int) -> Void
    let onReorder: (String, Bool) -> Void

    @Environment(\.kekulvPalette) private var palette
    @State private var isDropTargeted = false
    @State private var dropPlacement: ProviderDropPlacement = .after

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(alignment: .center, spacing: 8) {
                if showsQuickToggle {
                    Toggle(
                        isOn: Binding(
                            get: { row.enabled },
                            set: { onSetEnabled($0) }
                        )
                    ) {
                        Text(row.enabled ? "停用 \(row.name)" : "启用 \(row.name)")
                    }
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .disabled(isSaving)
                    .accessibilityLabel(row.enabled ? "停用 \(row.name)" : "启用 \(row.name)")
                } else {
                    StatusBadge(
                        text: row.statusText,
                        systemImage: row.enabled ? "checkmark.circle.fill" : "pause.circle",
                        color: row.enabled ? palette.success : palette.textSecondary
                    )
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.name)
                        .font(.callout.weight(.semibold))
                        .lineLimit(1)
                    Text(row.id)
                        .font(.caption.monospaced())
                        .foregroundStyle(palette.textMuted)
                        .lineLimit(1)
                }
                .help(row.name)
                Spacer(minLength: 4)
            }
            HStack(spacing: 10) {
                Label(row.priorityText, systemImage: "arrow.up.arrow.down")
                Label(row.protocolDisplayName, systemImage: "network")
                Label("映射 \(row.mappingCount)", systemImage: "arrow.left.arrow.right")
                Label(modelCatalogSummary, systemImage: "square.grid.2x2")
                Spacer(minLength: 0)
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(palette.textSecondary)
            HStack(spacing: 6) {
                ProviderRowIconButton(
                    systemImage: "chevron.up",
                    label: "上移入口：\(row.name)",
                    help: "上移同优先级入口顺序",
                    disabled: !canMoveUp,
                    action: { onMove(-1) }
                )
                ProviderRowIconButton(
                    systemImage: "chevron.down",
                    label: "下移入口：\(row.name)",
                    help: "下移同优先级入口顺序",
                    disabled: !canMoveDown,
                    action: { onMove(1) }
                )
                Button(action: onEdit) {
                    Label("编辑", systemImage: "square.and.pencil")
                        .labelStyle(.iconOnly)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .frame(minWidth: 44, minHeight: 44)
                .help("编辑入口")
                .accessibilityLabel("编辑入口：\(row.name)")
                ProviderFetchModelsButton(
                    endpointName: row.name,
                    isFetching: isFetchingModels,
                    compact: true,
                    action: onFetchModels
                )
                Button(role: .destructive, action: onDelete) {
                    Label("删除", systemImage: "trash")
                        .labelStyle(.iconOnly)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .frame(minWidth: 44, minHeight: 44)
                .help("删除入口")
                .accessibilityLabel("删除入口：\(row.name)")
                Spacer(minLength: 0)
            }
            if row.stickyGroupText != "-" {
                Label("粘性分组：\(row.stickyGroupText)", systemImage: "link")
                    .font(.caption)
                    .foregroundStyle(palette.textSecondary)
                    .lineLimit(2)
            }
            if isSaving {
                Label("保存中…", systemImage: "arrow.clockwise")
                    .font(.caption)
                    .foregroundStyle(palette.textSecondary)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(isSelected ? palette.active : .clear)
                .padding(.horizontal, 4)
        )
        .overlay {
            if isDropTargeted {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(palette.brand.opacity(0.72), lineWidth: 1.2)
                    .padding(.horizontal, 4)
            }
        }
        .contentShape(Rectangle())
        .onDrag {
            draggedProviderID = row.id
            return NSItemProvider(object: row.id as NSString)
        } preview: {
            ProviderRowDragPreview(row: row)
        }
        .onDrop(
            of: [UTType.utf8PlainText],
            delegate: ProviderRowDropDelegate(
                targetID: row.id,
                draggedProviderID: $draggedProviderID,
                isTargeted: $isDropTargeted,
                placement: $dropPlacement,
                rowHeight: 88,
                onReorder: onReorder
            )
        )
        .onTapGesture(perform: onSelect)
        .contextMenu {
            Button("编辑", action: onEdit)
            Button(row.enabled ? "停用" : "启用") { onSetEnabled(!row.enabled) }
                .disabled(isSaving)
            Button("复制 API 地址", action: onCopyBaseURL)
            Divider()
            Button("删除", role: .destructive, action: onDelete)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("入口：\(row.name)")
        .accessibilityHint("单击选择；按住鼠标左键可调整顺序；可通过右键菜单编辑")
        .accessibilityValue(Text(isSelected ? "已选中，\(row.statusText)" : row.statusText))
    }

    private var modelCatalogSummary: String {
        let status = row.catalog.status.isEmpty ? "未获取" : row.catalog.status
        let count = row.catalog.uniqueModels.count
        return count > 0 ? "\(status) · \(count) 个" : status
    }
}

/// 每个入口只有一个拖拽源和一个放置目标。所有列仍属于同一行，因此在任意
/// 非滚动条位置按住左键都可拖动；没有原生 Table 的范围选择手势参与竞争。
private struct ProviderAccountListRow: View {
    let row: EndpointDisplayRow
    let widths: ProviderTableColumnWidths
    let isSelected: Bool
    let isHovered: Bool
    let showsQuickToggle: Bool
    let isSaving: Bool
    let isFetchingModels: Bool
    @Binding var draggedProviderID: String?
    let onSelect: () -> Void
    let onHover: (Bool) -> Void
    let onEdit: () -> Void
    let onSetEnabled: (Bool) -> Void
    let onFetchModels: () -> Void
    let onCopyBaseURL: () -> Void
    let onDelete: () -> Void
    let canMoveUp: Bool
    let canMoveDown: Bool
    let onMove: (Int) -> Void
    let onReorder: (String, Bool) -> Void

    @Environment(\.kekulvPalette) private var palette
    @State private var isDropTargeted = false
    @State private var dropPlacement: ProviderDropPlacement = .after

    var body: some View {
        HStack(spacing: 0) {
            statusCell
            nameCell
            columnCell(width: widths.priority) {
                Text(row.priorityText)
                    .font(.body.monospacedDigit().weight(.bold))
                    .foregroundStyle(palette.brand)
            }
            columnCell(width: widths.protocolName) {
                Text(row.protocolDisplayName)
                    .font(.callout)
                    .foregroundStyle(palette.textSecondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(row.protocolDisplayName)
            }
            columnCell(width: widths.mapping) {
                Text("\(row.mappingCount) 条")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(palette.brand)
            }
            columnCell(width: widths.models) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(modelCatalogSummary)
                        .font(.callout)
                        .foregroundStyle(row.catalog.status == "获取失败" ? palette.danger : palette.textSecondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    if !row.catalog.updatedAt.isEmpty {
                        Text(row.catalog.updatedAt)
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(palette.textMuted)
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .help(row.catalog.updatedAt)
                    }
                }
            }
            columnCell(width: widths.actions) {
                HStack(spacing: 6) {
                    ProviderRowIconButton(
                        systemImage: "chevron.up",
                        label: "上移入口：\(row.name)",
                        help: "上移同优先级入口顺序",
                        disabled: !canMoveUp,
                        action: { onMove(-1) }
                    )
                    ProviderRowIconButton(
                        systemImage: "chevron.down",
                        label: "下移入口：\(row.name)",
                        help: "下移同优先级入口顺序",
                        disabled: !canMoveDown,
                        action: { onMove(1) }
                    )
                    ProviderRowIconButton(
                        systemImage: "square.and.pencil",
                        label: "编辑入口：\(row.name)",
                        help: "编辑入口「\(row.name)」",
                        disabled: false,
                        action: onEdit
                    )
                    .help("编辑入口「\(row.name)」")
                    ProviderFetchModelsButton(
                        endpointName: row.name,
                        isFetching: isFetchingModels,
                        action: onFetchModels
                    )
                    Button(role: .destructive, action: onDelete) {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .frame(minWidth: 28, minHeight: 28)
                    .help("删除入口「\(row.name)」")
                    .accessibilityLabel("删除入口：\(row.name)")
                }
                .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
        .frame(width: widths.total, height: ProviderTableColumn.rowHeight, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(isSelected ? palette.active : (isHovered ? palette.hover : .clear))
                .padding(.horizontal, 4)
        )
        .overlay {
            if isDropTargeted {
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .fill(palette.brand.opacity(0.06))
                    .padding(.horizontal, 4)
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .stroke(palette.brand.opacity(0.72), lineWidth: 1.2)
                    .padding(.horizontal, 4)
            }
        }
        .overlay(alignment: .top) {
            if isDropTargeted && dropPlacement == .before {
                ProviderDropInsertionPreview(placement: .before)
                    .allowsHitTesting(false)
                    .zIndex(2)
            }
        }
        .overlay(alignment: .bottom) {
            if isDropTargeted && dropPlacement == .after {
                ProviderDropInsertionPreview(placement: .after)
                    .allowsHitTesting(false)
                    .zIndex(2)
            }
        }
        .overlay(alignment: .bottom) {
            if !isDropTargeted {
                Rectangle()
                    .fill(palette.borderSubtle)
                    .frame(height: 0.5)
                    .padding(.horizontal, 8)
            }
        }
        .contentShape(Rectangle())
        .onDrag {
            draggedProviderID = row.id
            return NSItemProvider(object: row.id as NSString)
        } preview: {
            ProviderRowDragPreview(row: row)
        }
        .onDrop(
            of: [UTType.utf8PlainText],
            delegate: ProviderRowDropDelegate(
                targetID: row.id,
                draggedProviderID: $draggedProviderID,
                isTargeted: $isDropTargeted,
                placement: $dropPlacement,
                rowHeight: ProviderTableColumn.rowHeight,
                onReorder: onReorder
            )
        )
        .onHover(perform: onHover)
        .onTapGesture(perform: onSelect)
        .contextMenu {
            Button("编辑", action: onEdit)
            Button(row.enabled ? "停用" : "启用") { onSetEnabled(!row.enabled) }
                .disabled(isSaving)
            Button("复制 API 地址", action: onCopyBaseURL)
            Divider()
            Button("删除", role: .destructive, action: onDelete)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("入口：\(row.name)")
        .accessibilityHint("单击选择；按住鼠标左键可拖到其他行上方或下方；可通过右键菜单编辑")
        .accessibilityValue(Text(isSelected ? "已选中，\(row.statusText)" : row.statusText))
    }

    @ViewBuilder
    private var statusCell: some View {
        if showsQuickToggle {
            columnCell(width: widths.status) {
                HStack(spacing: 7) {
                    Toggle(
                        isOn: Binding(
                            get: { row.enabled },
                            set: { enabled in onSetEnabled(enabled) }
                        )
                    ) {
                        Text(row.enabled ? "停用 \(row.name)" : "启用 \(row.name)")
                    }
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .disabled(isSaving)
                    .accessibilityLabel(row.enabled ? "停用 \(row.name)" : "启用 \(row.name)")
                    .accessibilityHint("立即保存入口状态")
                    .accessibilityValue(Text(isSaving ? "保存中" : row.statusText))
                    if isSaving {
                        ProgressView()
                            .controlSize(.small)
                            .accessibilityHidden(true)
                    }
                }
            }
        } else {
            columnCell(width: widths.status) {
                StatusBadge(
                    text: row.statusText,
                    systemImage: row.enabled ? "checkmark.circle.fill" : "pause.circle",
                    color: row.enabled ? .green : .secondary
                )
            }
        }
    }

    private var nameCell: some View {
        columnCell(width: widths.name) {
            HStack(spacing: 7) {
                Image(systemName: "line.3.horizontal")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: 30, height: 30)
                    .background(
                        Capsule(style: .continuous)
                            .fill(Color.secondary.opacity(0.10))
                    )
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.name)
                        .font(.callout.weight(.semibold))
                        .lineLimit(1)
                    Text(row.id)
                        .font(.caption.monospaced())
                        .foregroundStyle(palette.textMuted)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                        .help(row.id)
                }
                .help("按住这一行任意位置拖动调整入口顺序")
                Spacer(minLength: 0)
            }
        }
    }

    private var modelCatalogSummary: String {
        let status = row.catalog.status.isEmpty ? "未获取" : row.catalog.status
        let count = row.catalog.uniqueModels.count
        return count > 0 ? "\(status) · \(count) 个" : status
    }

    private func columnCell<Content: View>(
        width: CGFloat,
        @ViewBuilder content: () -> Content
    ) -> some View {
        content()
            .padding(.horizontal, 8)
            .frame(width: width, height: ProviderTableColumn.rowHeight, alignment: .leading)
    }
}

/// 入口级模型目录操作。模型目录属于单个上游入口，放在对应行尾能让
/// 用户明确知道“获取模型”作用于哪一个入口，也允许多个入口分别加载。
private struct ProviderFetchModelsButton: View {
    let endpointName: String
    let isFetching: Bool
    var compact = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            if compact {
                if isFetching {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    Image(systemName: "arrow.clockwise")
                }
            } else {
                HStack(spacing: 5) {
                    if isFetching {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Image(systemName: "arrow.clockwise")
                    }
                    Text(isFetching ? "获取中…" : "获取模型")
                }
            }
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(isFetching)
        .frame(minWidth: compact ? 44 : 104, minHeight: compact ? 44 : 30)
        .help(isFetching ? "正在从入口「\(endpointName)」获取模型" : "从入口「\(endpointName)」读取当前可用模型目录")
        .accessibilityLabel(isFetching ? "正在获取入口 \(endpointName) 的模型" : "获取入口 \(endpointName) 的模型")
        .accessibilityHint("读取该入口当前可用的模型目录")
    }
}

private struct ProviderRowIconButton: View {
    let systemImage: String
    let label: String
    let help: String
    let disabled: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .frame(width: 18, height: 18)
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(disabled)
        .help(help)
        .accessibilityLabel(label)
    }
}

/// `dropDestination` only exposes the pointer location after the drop.  A
/// `DropDelegate` receives `dropUpdated` callbacks throughout the drag session,
/// which lets the row move its insertion line as the pointer crosses the
/// midpoint.  The payload is intentionally accepted only for drags started by
/// this table; external text drops must never be interpreted as Provider IDs.
private struct ProviderRowDropDelegate: DropDelegate {
    let targetID: String
    @Binding var draggedProviderID: String?
    @Binding var isTargeted: Bool
    @Binding var placement: ProviderDropPlacement
    let rowHeight: CGFloat
    let onReorder: (String, Bool) -> Void

    func validateDrop(info: DropInfo) -> Bool {
        guard let draggedID = draggedProviderID, draggedID != targetID else { return false }
        return info.hasItemsConforming(to: [.utf8PlainText])
    }

    func dropEntered(info: DropInfo) {
        guard validateDrop(info: info) else { return }
        isTargeted = true
        updatePlacement(for: info)
    }

    func dropUpdated(info: DropInfo) -> DropProposal? {
        guard validateDrop(info: info) else {
            isTargeted = false
            return DropProposal(operation: .forbidden)
        }
        isTargeted = true
        updatePlacement(for: info)
        return DropProposal(operation: .move)
    }

    func dropExited(info: DropInfo) {
        isTargeted = false
    }

    func performDrop(info: DropInfo) -> Bool {
        guard let draggedID = draggedProviderID, draggedID != targetID else {
            isTargeted = false
            return false
        }
        updatePlacement(for: info)
        onReorder(draggedID, placement == .after)
        draggedProviderID = nil
        isTargeted = false
        return true
    }

    private func updatePlacement(for info: DropInfo) {
        placement = info.location.y >= rowHeight / 2 ? .after : .before
    }
}

/// Drag previews should carry the same row identity that the drop target is
/// previewing.  A small, complete summary is easier to follow than the old
/// one-line "移动入口" label, while deliberately omitting the API key and
/// full URL so the preview cannot expose credentials or sensitive endpoints.
private struct ProviderRowDragPreview: View {
    let row: EndpointDisplayRow
    @Environment(\.kekulvPalette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                Image(systemName: "line.3.horizontal")
                    .foregroundStyle(palette.brand)
                    .accessibilityHidden(true)
                Text(row.name)
                    .font(.callout.weight(.semibold))
                    .lineLimit(1)
                Spacer(minLength: 8)
                Label(row.statusText, systemImage: row.enabled ? "checkmark.circle.fill" : "pause.circle")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(row.enabled ? palette.success : palette.textSecondary)
            }
            HStack(spacing: 10) {
                Text("优先级 \(row.priorityText)")
                Text(row.protocolDisplayName)
                Text("映射 \(row.mappingCount)")
                Spacer(minLength: 0)
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(palette.textSecondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(width: 380, alignment: .leading)
        .background(palette.raised, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(palette.brand.opacity(0.42), lineWidth: 1)
        )
        .shadow(color: .black.opacity(0.16), radius: 8, y: 3)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("正在移动入口 \(row.name)，\(row.statusText)，优先级 \(row.priorityText)，\(row.protocolDisplayName)，映射 \(row.mappingCount) 个")
    }
}

/// A row-local insertion affordance: the line spans the complete table width,
/// with a small semantic label so placement is not communicated by colour
/// alone.  It is an overlay and never captures the row's normal hit testing.
private struct ProviderDropInsertionPreview: View {
    let placement: ProviderDropPlacement
    @Environment(\.kekulvPalette) private var palette

    var body: some View {
        ZStack {
            // A max-width shape is used instead of two intrinsic-less
            // capsules in an HStack; the latter can collapse to zero width
            // when SwiftUI proposes an unspecified size to an overlay.
            Rectangle()
                .fill(palette.brand)
                .frame(height: 3)
                .frame(maxWidth: .infinity)
            Label(placement.label, systemImage: placement.systemImage)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(palette.brand)
                .padding(.horizontal, 7)
                .padding(.vertical, 4)
                .background(palette.raised.opacity(0.96), in: Capsule())
                .overlay(Capsule().stroke(palette.brand.opacity(0.35), lineWidth: 0.8))
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 4)
        .offset(y: placement == .before ? -1.5 : 1.5)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

private func timeoutSummary(_ seconds: Double?) -> String {
    seconds.map { "\(String($0))s" } ?? "客户端决定"
}

struct MappingTable: View {
    let rows: [MappingDisplayRow]
    @Binding var selection: Set<String>
    var onEdit: (MappingDisplayRow) -> Void = { _ in }
    var onDeleteIDs: (Set<String>) -> Void = { _ in }

    var body: some View {
        if rows.isEmpty {
            EmptyStateView(title: "暂无模型映射。", systemImage: "arrow.left.arrow.right")
        } else {
            Table(rows, selection: $selection) {
                TableColumn("客户端模型") { row in
                    Text(row.clientPattern).lineLimit(1)
                }
                TableColumn("上游模型") { row in
                    Text(row.upstreamModel).lineLimit(1)
                }
                TableColumn("Thinking") { row in
                    Text(row.thinking.rawValue)
                }
                TableColumn("上下文") { row in
                    Text(row.context.displayName)
                }
                TableColumn("首个超时") { row in
                    Text(timeoutSummary(row.failoverTimeoutSeconds)).monospacedDigit()
                }
            }
            .kekulvTableSurface()
            // 双击 = 编辑;右键 = 编辑/删除。
            .contextMenu(forSelectionType: String.self) { ids in
                if ids.count == 1, let row = rows.first(where: { $0.id == ids.first }) {
                    Button("编辑") { onEdit(row) }
                }
                if !ids.isEmpty {
                    Button("删除", role: .destructive) { onDeleteIDs(ids) }
                }
            } primaryAction: { ids in
                if ids.count == 1, let row = rows.first(where: { $0.id == ids.first }) {
                    onEdit(row)
                }
            }
            .frame(height: adaptiveTableHeight(rows: rows.count, max: 280))
            .onDeleteCommand {
                if !selection.isEmpty { onDeleteIDs(selection) }
            }
        }
    }
}
