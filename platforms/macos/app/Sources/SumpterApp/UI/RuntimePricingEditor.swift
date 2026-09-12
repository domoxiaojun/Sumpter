import SwiftUI

/// Shared editor for the runtime model price catalog.
///
/// The editor is opened from a Provider entry. `endpointID` scopes the visible
/// rows to that entry (global fallback rows remain visible and editable), while
/// the full draft is retained for the server-side revision-safe update so
/// editing one entry never deletes prices belonging to another entry.
struct RuntimePricingEditor: View {
    private struct ConfiguredModel: Identifiable, Hashable {
        let endpointID: String
        let endpointName: String
        let modelKey: String

        var id: String { "\(endpointID)/\(modelKey)" }
    }

    @ObservedObject var model: AppModel
    let endpointID: String?
    @Environment(\.dismiss) private var dismiss

    @State private var currency = "USD"
    @State private var rows: [AdminWire.RuntimeModelPrice] = []
    @State private var dirty = false
    @State private var savingRevision: Int?
    @State private var validationMessage: String?

    init(model: AppModel, endpointID: String? = nil) {
        self.model = model
        self.endpointID = endpointID
    }

    private var scopedEndpointName: String? {
        guard let endpointID else { return nil }
        return model.config.endpoint(id: endpointID).map { $0.name.isEmpty ? $0.id : $0.name }
    }

    private var visibleRows: [AdminWire.RuntimeModelPrice] {
        guard let endpointID else { return rows }
        // Global fallback prices are effective for this entry, so keep them in
        // view; other endpoint-specific rows remain in `rows` for lossless save.
        return rows.filter { $0.endpointID == nil || $0.endpointID == endpointID }
    }

    private var configuredModels: [ConfiguredModel] {
        var result: [ConfiguredModel] = []
        let endpoints = endpointID.map { id in
            model.config.endpoints.filter { $0.id == id }
        } ?? model.config.endpoints
        for endpoint in endpoints {
            var models = Set(endpoint.catalog.uniqueModels.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) })
            endpoint.mappings.forEach { mapping in
                let client = mapping.clientPattern.rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
                if !client.isEmpty { models.insert(client) }
            }
            for modelKey in models where !modelKey.isEmpty {
                result.append(ConfiguredModel(endpointID: endpoint.id, endpointName: endpoint.name.isEmpty ? endpoint.id : endpoint.name, modelKey: modelKey))
            }
        }
        return result.sorted { ($0.endpointName + "/" + $0.modelKey).localizedStandardCompare($1.endpointName + "/" + $1.modelKey) == .orderedAscending }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(scopedEndpointName.map { "成本价格 · \($0)" } ?? "成本价格")
                        .font(.title2.weight(.semibold))
                    Text(scopedEndpointName == nil
                        ? "按入口 + 精确模型 key 维护每百万 Token 的价格；未配置入口专属价格时使用全局回退。"
                        : "维护此入口的模型价格；未配置专属价格时继续使用全局回退。")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                TextField("货币", text: $currency)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 76)
                    .textCase(.uppercase)
                    .onChange(of: currency) { _, _ in dirty = true }
            }

            HStack(spacing: 8) {
                Text("价格表")
                    .font(.headline)
                Text("共 \(visibleRows.count) 个模型")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
                Button {
                    addRow()
                } label: {
                    Label("添加模型", systemImage: "plus")
                }
                .frame(minHeight: 44)
                if !configuredModels.isEmpty {
                    Menu {
                        ForEach(configuredModels) { configured in
                            Button {
                                addConfiguredModel(configured)
                            } label: {
                                Text("\(configured.endpointName) · \(configured.modelKey)")
                            }
                        }
                    } label: {
                        Label("从当前入口添加", systemImage: "list.bullet.rectangle")
                    }
                    .frame(minHeight: 44)
                }
            }

            if visibleRows.isEmpty {
                EmptyStateView(title: "尚未配置模型价格", systemImage: "yensign.circle")
                    .frame(maxWidth: .infinity, minHeight: 150)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        ForEach(visibleRows) { row in
                            pricingRow(row)
                        }
                    }
                    .padding(.vertical, 2)
                }
                .frame(maxHeight: 390)
            }

            if let validationMessage {
                Label(validationMessage, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                if model.runtimePricing != nil {
                    Text("单位：每百万 Token（货币金额）")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
                Button("取消") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                    .frame(minWidth: 72, minHeight: 44)
                Button {
                    save()
                } label: {
                    if savingRevision != nil {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("保存价格表")
                    }
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .frame(minWidth: 112, minHeight: 44)
                .disabled(!dirty || pricingDraft == nil || savingRevision != nil)
            }
        }
        .padding(20)
        .frame(minWidth: 760, minHeight: 480)
        .task {
            syncDraftIfNeeded()
            if model.runtimePricing == nil {
                model.refreshRuntimeMaintenance()
            }
        }
        .onChange(of: model.runtimePricing) { _, pricing in
            guard let pricing else { return }
            if let expected = savingRevision, pricing.revision > expected {
                savingRevision = nil
                dirty = false
                syncDraftIfNeeded(force: true)
            } else {
                syncDraftIfNeeded()
            }
        }
        .onChange(of: model.runtimeV2Error) { _, error in
            guard let error, error.contains("价格表更新失败") else { return }
            savingRevision = nil
            validationMessage = error
        }
    }

    private func pricingRow(_ row: AdminWire.RuntimeModelPrice) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                if let endpointID, row.endpointID == endpointID {
                    Text(scopedEndpointName ?? endpointID)
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .frame(minWidth: 150, idealWidth: 190, alignment: .leading)
                } else {
                    Picker("入口", selection: endpointBinding(id: row.id)) {
                        Text("全局回退").tag(String?.none)
                        ForEach(model.config.endpoints) { endpoint in
                            Text(endpoint.name.isEmpty ? endpoint.id : endpoint.name).tag(String?.some(endpoint.id))
                        }
                    }
                    .pickerStyle(.menu)
                    .frame(minWidth: 150, idealWidth: 190)
                }
                TextField("模型 key", text: modelKeyBinding(id: row.id))
                    .textFieldStyle(.roundedBorder)
                    .font(.body.monospaced())
                    .help(row.modelKey)
                Text(RuntimeEventDisplay.dateTime(Date(timeIntervalSinceReferenceDate: row.effectiveFrom)))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer(minLength: 0)
                Button(role: .destructive) {
                    dirty = true
                    rows.removeAll { $0.id == row.id }
                } label: {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
                .frame(minWidth: 44, minHeight: 44)
                .help("删除价格 \(row.modelKey)")
                .accessibilityLabel("删除价格 \(row.modelKey)")
            }
            LazyVGrid(columns: [
                GridItem(.flexible(minimum: 130), alignment: .leading),
                GridItem(.flexible(minimum: 130), alignment: .leading),
                GridItem(.flexible(minimum: 130), alignment: .leading),
                GridItem(.flexible(minimum: 130), alignment: .leading),
            ], alignment: .leading, spacing: 10) {
                priceField(title: "输入", id: row.id, field: .input)
                priceField(title: "输出", id: row.id, field: .output)
                priceField(title: "缓存读取", id: row.id, field: .cacheRead)
                priceField(title: "缓存写入", id: row.id, field: .cacheCreation)
            }
        }
        .padding(12)
        .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
    }

    private func priceField(title: String, id: Int, field: PriceField) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
            TextField("例如 5.00", text: priceBinding(id: id, field: field))
                .textFieldStyle(.roundedBorder)
                .font(.body.monospacedDigit())
        }
    }

    private enum PriceField {
        case input
        case output
        case cacheRead
        case cacheCreation
    }

    private func syncDraftIfNeeded(force: Bool = false) {
        guard (force || !dirty), let pricing = model.runtimePricing else { return }
        currency = pricing.currency
        rows = pricing.prices
        validationMessage = nil
    }

    private func addRow() {
        dirty = true
        let nextID = (rows.map(\.id).max() ?? 0) + 1
        rows.append(AdminWire.RuntimeModelPrice(
            id: nextID,
            endpointID: endpointID,
            modelKey: "new-model",
            effectiveFrom: Date().timeIntervalSinceReferenceDate,
            effectiveTo: nil,
            inputPerMillionMicros: nil,
            outputPerMillionMicros: nil,
            cacheReadPerMillionMicros: nil,
            cacheCreationPerMillionMicros: nil
        ))
    }

    private func addConfiguredModel(_ configured: ConfiguredModel) {
        guard !rows.contains(where: { $0.endpointID == configured.endpointID && $0.modelKey == configured.modelKey }) else { return }
        dirty = true
        let nextID = (rows.map(\.id).max() ?? 0) + 1
        rows.append(AdminWire.RuntimeModelPrice(
            id: nextID,
            endpointID: configured.endpointID,
            modelKey: configured.modelKey,
            effectiveFrom: Date().timeIntervalSinceReferenceDate,
            effectiveTo: nil,
            inputPerMillionMicros: nil,
            outputPerMillionMicros: nil,
            cacheReadPerMillionMicros: nil,
            cacheCreationPerMillionMicros: nil
        ))
    }

    private var pricingDraft: AdminWire.RuntimePricingUpdate? {
        guard let pricing = model.runtimePricing else { return nil }
        let normalizedCurrency = currency.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        guard normalizedCurrency.count == 3,
              rows.allSatisfy({
                  !$0.modelKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                      && ($0.inputPerMillionMicros ?? 0) >= 0
                      && ($0.outputPerMillionMicros ?? 0) >= 0
                      && ($0.cacheReadPerMillionMicros ?? 0) >= 0
                      && ($0.cacheCreationPerMillionMicros ?? 0) >= 0
              }) else { return nil }
        return AdminWire.RuntimePricingUpdate(
            expectedRevision: pricing.revision,
            currency: normalizedCurrency,
            prices: rows
        )
    }

    private func save() {
        guard let draft = pricingDraft else {
            validationMessage = "货币必须是 3 个字母；模型 key 不能为空；价格只能是非负整数。"
            return
        }
        validationMessage = nil
        savingRevision = draft.expectedRevision
        model.updateRuntimePricing(draft)
    }

    private func modelKeyBinding(id: Int) -> Binding<String> {
        Binding(
            get: { rows.first(where: { $0.id == id })?.modelKey ?? "" },
            set: { value in
                guard let index = rows.firstIndex(where: { $0.id == id }) else { return }
                dirty = true
                rows[index].modelKey = value
            }
        )
    }

    private func endpointBinding(id: Int) -> Binding<String?> {
        Binding(
            get: { rows.first(where: { $0.id == id })?.endpointID },
            set: { value in
                guard let index = rows.firstIndex(where: { $0.id == id }) else { return }
                dirty = true
                rows[index].endpointID = value
            }
        )
    }

    private func priceBinding(id: Int, field: PriceField) -> Binding<String> {
        Binding(
            get: {
                guard let row = rows.first(where: { $0.id == id }) else { return "" }
                let value: Int?
                switch field {
                case .input: value = row.inputPerMillionMicros
                case .output: value = row.outputPerMillionMicros
                case .cacheRead: value = row.cacheReadPerMillionMicros
                case .cacheCreation: value = row.cacheCreationPerMillionMicros
                }
                guard let value else { return "" }
                var text = String(format: "%.6f", Double(value) / 1_000_000)
                while text.last == "0" { text.removeLast() }
                if text.last == "." { text.removeLast() }
                return text
            },
            set: { text in
                guard let index = rows.firstIndex(where: { $0.id == id }) else { return }
                let normalized = text.trimmingCharacters(in: .whitespacesAndNewlines)
                guard normalized.isEmpty || Self.priceMicros(normalized) != nil else { return }
                dirty = true
                let value = normalized.isEmpty ? nil : Self.priceMicros(normalized)
                switch field {
                case .input: rows[index].inputPerMillionMicros = value
                case .output: rows[index].outputPerMillionMicros = value
                case .cacheRead: rows[index].cacheReadPerMillionMicros = value
                case .cacheCreation: rows[index].cacheCreationPerMillionMicros = value
                }
            }
        )
    }

    private static func priceMicros(_ text: String) -> Int? {
        guard let amount = Double(text), amount.isFinite, amount >= 0 else { return nil }
        let micros = (amount * 1_000_000).rounded()
        guard micros <= Double(Int.max) else { return nil }
        return Int(micros)
    }
}
