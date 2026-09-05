import SumpterCore
import SwiftUI

struct RetryPolicySheet: View {
    @ObservedObject var model: AppModel
    let onClose: () -> Void
    @State private var responseTimeout: String
    @State private var streamIdle: String
    @State private var max500Retries: String
    @State private var failoverOn500: Bool
    @State private var retryDelaySeconds: String
    @State private var passThroughRetryDelay: Bool
    @State private var deferredRounds: String
    @State private var retryMaxSeconds: String
    @State private var sessionStickyRetries: String
    @State private var submission = SubmissionState.idle

    init(model: AppModel, onClose: @escaping () -> Void) {
        self.model = model
        self.onClose = onClose
        let retry = model.config.retry
        _responseTimeout = State(initialValue: retry.responseTimeoutSeconds.map { String($0) } ?? "")
        _streamIdle = State(initialValue: retry.streamIdleTimeoutSeconds.map { String($0) } ?? "")
        _max500Retries = State(initialValue: "\(retry.max500Retries)")
        _failoverOn500 = State(initialValue: retry.failoverOn500)
        _retryDelaySeconds = State(initialValue: retry.retryDelaySeconds.map { String($0) } ?? "")
        _passThroughRetryDelay = State(initialValue: retry.passThroughRetryDelay)
        _deferredRounds = State(initialValue: "\(retry.maxDeferredRounds)")
        _retryMaxSeconds = State(initialValue: "\(retry.maxRetryDurationSeconds)")
        _sessionStickyRetries = State(initialValue: "\(retry.sessionStickyRetries)")
    }

    var body: some View {
        SheetShell(
            title: "编辑全局转发与重试策略",
            primaryTitle: submission.isSubmitting ? "保存中..." : "保存",
            primaryDisabled: submission.isSubmitting,
            onCancel: onClose,
            onSubmit: submit
        ) {
            VStack(alignment: .leading, spacing: 12) {
                FormLine(title: "首响应截止（秒）") {
                    TextField("客户端决定", text: $responseTimeout)
                        .frame(width: 120)
                }
                Text("连接、TLS 握手到首个响应头的总等待上限；留空由客户端决定。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 122)
                FormLine(title: "流式空闲截止（秒）") {
                    TextField("客户端决定", text: $streamIdle)
                        .frame(width: 120)
                }
                Text("首个响应后，两次流式输出之间的最大空闲时间；留空不限制。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 122)
                Text("HTTP 500 处理")
                    .font(.headline)
                    .padding(.top, 4)
                FormLine(title: "500 失败后切换入口") {
                    HStack(spacing: 8) {
                        Toggle("", isOn: $failoverOn500)
                            .labelsHidden()
                            .toggleStyle(.switch)
                            .accessibilityLabel("HTTP 500 失败后切换入口")
                        Text(failoverOn500 ? "已开启" : "已关闭")
                            .foregroundStyle(.secondary)
                    }
                }
                Text(failoverOn500
                    ? "开启：当前入口重试耗尽后，继续尝试下一个入口。"
                    : "关闭：当前入口重试耗尽后直接返回 HTTP 500，不会切换入口。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 122)
                FormLine(title: "入口内 500 重试") {
                    TextField("0", text: $max500Retries)
                        .frame(width: 160)
                }
                Text("仅针对同一入口连续收到的 HTTP 500；0 表示不额外重试。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 122)
                FormLine(title: "retry_delay 秒数") {
                    TextField("例如 2.5", text: $retryDelaySeconds)
                        .frame(width: 160)
                        .disabled(!passThroughRetryDelay)
                }
                FormLine(title: "透传 retry_delay") {
                    HStack(spacing: 8) {
                        Toggle("", isOn: $passThroughRetryDelay)
                            .labelsHidden()
                            .toggleStyle(.switch)
                            .accessibilityLabel("透传 retry_delay 与 Retry-After")
                        Text(passThroughRetryDelay ? "已开启" : "已关闭")
                            .foregroundStyle(.secondary)
                    }
                }
                Text("开启且填写秒数后，最终失败响应会带 retry_delay（秒）和 Retry-After；关闭则不返回这两个字段。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                FormLine(title: "故障重试最大轮数") {
                    TextField("0=不限制轮数", text: $deferredRounds)
                        .frame(width: 160)
                }
                FormLine(title: "跨轮最长时长（秒）") {
                    TextField("秒；两项均为0=无限", text: $retryMaxSeconds)
                        .frame(width: 160)
                }
                FormLine(title: "粘性入口额外重试") {
                    TextField("次数；0=立即切换", text: $sessionStickyRetries)
                        .frame(width: 160)
                }
                Text("同一次请求先完整重试当前粘性调度组（HTTP 500 使用上面的独立次数）；设置 2 就是首次非 500 可重试故障后再试 2 次，全部失败才访问其它组。其它组成功后立即成为该会话的新粘性入口。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                SheetErrorText(submission.errorText)
            }
        }
    }

    private func submit() {
        guard submission.begin() else { return }
        Task {
            do {
                try await model.updateRetryPolicyAndSave(
                    responseTimeoutText: responseTimeout,
                    streamIdleTimeoutText: streamIdle,
                    max500RetriesText: max500Retries,
                    failoverOn500: failoverOn500,
                    retryDelaySecondsText: retryDelaySeconds,
                    passThroughRetryDelay: passThroughRetryDelay,
                    maxDeferredRoundsText: deferredRounds,
                    maxRetryDurationSecondsText: retryMaxSeconds,
                    sessionStickyRetriesText: sessionStickyRetries
                )
                submission.succeed()
                onClose()
            } catch {
                submission.fail(error.localizedDescription)
            }
        }
    }
}

struct CatalogMappingSheet: View {
    @ObservedObject var model: AppModel
    let endpointID: String
    let onClose: () -> Void

    @State private var selection: Set<String> = []
    @State private var submission = SubmissionState.idle

    private var models: [String] {
        model.unmappedCatalogModels(endpointID: endpointID)
    }

    var body: some View {
        SheetShell(
            title: "从已知模型添加映射",
            primaryTitle: submission.isSubmitting ? "添加中..." : "添加所选 (\(selection.count))",
            primaryDisabled: submission.isSubmitting || selection.isEmpty,
            onCancel: onClose,
            onSubmit: submit
        ) {
            VStack(alignment: .leading, spacing: 10) {
                Text("勾选要加成路由的模型;客户端模型与上游模型同名直连,Thinking 透传、opus/fable 开 1M,可事后编辑。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button(selection.count == models.count ? "全不选" : "全选") {
                        selection = selection.count == models.count ? [] : Set(models)
                    }
                    .disabled(models.isEmpty)
                    Spacer()
                    Text("\(models.count) 个未映射")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if models.isEmpty {
                    EmptyStateView(title: "没有未映射的已知模型", systemImage: "checkmark.circle")
                } else {
                    Table(models.map(CatalogModelRow.init), selection: $selection) {
                        TableColumn("模型") { row in
                            Text(row.displayName).lineLimit(1)
                        }
                    }
                    .sumpterTableSurface()
                    .frame(height: 240)
                }
                SheetErrorText(submission.errorText)
            }
        }
    }

    private func submit() {
        guard submission.begin() else { return }
        let chosen = selection
        Task {
            do {
                try await model.addProviderMappingsFromCatalog(endpointID: endpointID, models: chosen)
                submission.succeed()
                onClose()
            } catch {
                submission.fail(error.localizedDescription)
            }
        }
    }
}

private struct CatalogModelRow: Identifiable, Hashable {
    /// 上游模型目录返回的真实 id —— selection 与上游调用均使用它。
    let id: String
    /// 展示名与真实 id 一致。
    let displayName: String
    init(_ id: String) {
        self.id = id
        displayName = id
    }
}

struct MappingEditorSheet: View {
    @ObservedObject var model: AppModel
    let endpointID: String
    let mapping: MappingDisplayRow?
    let onClose: () -> Void

    @State private var clientPattern: String
    @State private var upstreamModel: String
    @State private var thinking: ThinkingMode
    @State private var context: ContextMode
    @State private var failoverTimeout: String
    @State private var submission = SubmissionState.idle

    init(model: AppModel, endpointID: String, mapping: MappingDisplayRow?, onClose: @escaping () -> Void) {
        self.model = model
        self.endpointID = endpointID
        self.mapping = mapping
        self.onClose = onClose
        _clientPattern = State(initialValue: mapping?.clientPattern ?? "")
        _upstreamModel = State(initialValue: mapping?.upstreamModel ?? "")
        _thinking = State(initialValue: mapping?.thinking ?? .passthrough)
        _context = State(initialValue: mapping?.context ?? .standard)
        _failoverTimeout = State(initialValue: mapping?.failoverTimeoutSeconds.map { String($0) } ?? "")
    }

    var body: some View {
        SheetShell(
            title: mapping == nil ? "添加模型映射" : "编辑模型映射",
            primaryTitle: submission.isSubmitting ? "保存中..." : (mapping == nil ? "添加" : "保存"),
            primaryDisabled: submission.isSubmitting || !canSubmit,
            onCancel: onClose,
            onSubmit: submit
        ) {
            VStack(alignment: .leading, spacing: 12) {
                FormLine(title: "客户端模型") {
                    TextField("claude-sonnet-*", text: $clientPattern)
                }
                FormLine(title: "上游模型") {
                    TextField("留空则同名", text: $upstreamModel)
                }
                FormLine(title: "Thinking") {
                    Picker("Thinking", selection: $thinking) {
                        ForEach(ThinkingMode.allCases, id: \.rawValue) { mode in
                            Text(mode.rawValue).tag(mode)
                        }
                    }
                    .labelsHidden()
                }
                FormLine(title: "上下文") {
                    Picker("上下文", selection: $context) {
                        ForEach(ContextMode.allCases, id: \.rawValue) { mode in
                            Text(mode.displayName).tag(mode)
                        }
                    }
                    .labelsHidden()
                    .help("透传跟随客户端；1M 强制增加；剥离移除客户端 context-1m-*。")
                }
                FormLine(title: "首个超时") {
                    VStack(alignment: .leading, spacing: 2) {
                        TextField("留空则由客户端决定", text: $failoverTimeout)
                            .frame(width: 120)
                        Text("与全局首响应截止同时配置时，使用更早到达的截止时间。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                SheetErrorText(submission.errorText)
            }
        }
    }

    private var canSubmit: Bool {
        !clientPattern.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func submit() {
        guard submission.begin() else { return }
        Task {
            do {
                let timeout = try InputValidation.optionalPositiveDouble(failoverTimeout, field: "首个超时")
                if let mapping {
                    try await model.updateProviderMapping(
                        endpointID: endpointID,
                        mappingID: mapping.id,
                        clientPattern: clientPattern,
                        upstreamModel: upstreamModel,
                        thinking: thinking,
                        context: context,
                        failoverTimeoutSeconds: timeout
                    )
                } else {
                    try await model.addProviderMapping(
                        endpointID: endpointID,
                        clientPattern: clientPattern,
                        upstreamModel: upstreamModel,
                        thinking: thinking,
                        context: context,
                        failoverTimeoutSeconds: timeout
                    )
                }
                submission.succeed()
                onClose()
            } catch {
                submission.fail(error.localizedDescription)
            }
        }
    }
}

struct FeatureRuleEditorSheet: View {
    @ObservedObject var model: AppModel
    let rule: FeatureRule
    let onClose: () -> Void

    @State private var enabled: Bool
    @State private var endpointID: String
    @State private var targetModel: String
    @State private var effortChoice: String
    @State private var protocolChoice: String
    @State private var toolTypePrefix: String
    @State private var systemContains: String
    @State private var messagesContain: String
    @State private var modelEquals: String
    @State private var submission = SubmissionState.idle

    init(model: AppModel, rule: FeatureRule, onClose: @escaping () -> Void) {
        self.model = model
        self.rule = rule
        self.onClose = onClose
        _enabled = State(initialValue: rule.enabled)
        _endpointID = State(initialValue: rule.target.endpointID ?? "")
        _targetModel = State(initialValue: rule.target.model)
        _effortChoice = State(initialValue: rule.target.effortOverride?.rawValue ?? "")
        _protocolChoice = State(initialValue: rule.target.protocolOverride?.rawValue ?? "")
        _toolTypePrefix = State(initialValue: rule.match.toolTypePrefix ?? "")
        _systemContains = State(initialValue: rule.match.systemContains ?? "")
        _messagesContain = State(initialValue: rule.match.messagesContain ?? "")
        _modelEquals = State(initialValue: rule.match.modelEquals ?? "")
    }

    var body: some View {
        SheetShell(
            title: BuiltInFeatureRules.isBuiltIn(rule.id) ? "编辑预设分流目标" : "编辑分流规则",
            primaryTitle: submission.isSubmitting ? "保存中..." : "保存",
            primaryDisabled: submission.isSubmitting || targetModel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            onCancel: onClose,
            onSubmit: submit
        ) {
            VStack(alignment: .leading, spacing: 12) {
                FormLine(title: "名称") {
                    HStack(spacing: 6) {
                        if BuiltInFeatureRules.isBuiltIn(rule.id) {
                            Image(systemName: "lock.fill")
                                .foregroundStyle(.secondary)
                        }
                        Text(canonicalRule?.name ?? rule.name)
                            .font(.callout.weight(.semibold))
                    }
                }
                FormLine(title: "启用") {
                    Toggle("启用", isOn: $enabled)
                        .labelsHidden()
                }
                FormLine(title: "Provider 目标") {
                    VStack(alignment: .leading, spacing: 2) {
                        Picker("Provider 目标", selection: $endpointID) {
                            Text("候选序列（自动故障转移）").tag("")
                            ForEach(model.config.endpoints) { endpoint in
                                Text(endpoint.name.isEmpty ? endpoint.id : endpoint.name).tag(endpoint.id)
                            }
                        }
                        .labelsHidden()
                        .frame(width: 260, alignment: .leading)
                        Text(endpointID.isEmpty
                            ? "命中后按 Provider 候选序列自动故障转移。"
                            : "命中后固定使用该 Provider；停用或删除时自动退回候选序列。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                FormLine(title: "目标模型") {
                    HStack {
                        TextField("claude-haiku-4-5-20251001", text: $targetModel)
                        Menu {
                            if candidateModels.isEmpty {
                                Text("暂无候选")
                            } else {
                                ForEach(candidateModels, id: \.self) { item in
                                    Button(item) {
                                        targetModel = item
                                    }
                                }
                            }
                        } label: {
                            Label("候选", systemImage: "chevron.down.circle")
                        }
                        .disabled(candidateModels.isEmpty)
                    }
                }
                FormLine(title: "目标协议") {
                    Picker("目标协议", selection: $protocolChoice) {
                        Text("自动选择").tag("")
                        Text("Anthropic").tag(ProviderProtocol.anthropic.rawValue)
                        Text("OpenAI Responses").tag(ProviderProtocol.openaiResponses.rawValue)
                        Text("OpenAI Chat").tag(ProviderProtocol.openai.rawValue)
                    }
                    .labelsHidden()
                    .frame(width: 160, alignment: .leading)
                }
                FormLine(title: "Effort 覆盖") {
                    VStack(alignment: .leading, spacing: 2) {
                        Picker("Effort 覆盖", selection: $effortChoice) {
                            Text("跟随原请求").tag("")
                            ForEach(ReasoningEffort.allCases, id: \.rawValue) { effort in
                                Text("\(effort.displayName)（\(effort.rawValue)）").tag(effort.rawValue)
                            }
                        }
                        .labelsHidden()
                        .frame(width: 210, alignment: .leading)
                        Text("仅在命中这条路由时覆盖客户端 effort；跟随原请求不会改写。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Divider()
                if let canonicalRule {
                    VStack(alignment: .leading, spacing: 8) {
                        Label("预设识别条件由Sumpter维护，不可修改。", systemImage: "lock.fill")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .padding(.leading, 122)
                        readOnlyMatchRows(canonicalRule.match)
                    }
                } else {
                    FormLine(title: "Tool prefix") {
                        TextField("web_search", text: $toolTypePrefix)
                    }
                    FormLine(title: "System 包含") {
                        TextField("security monitor", text: $systemContains)
                    }
                    FormLine(title: "Messages 包含") {
                        TextField("Web page content:", text: $messagesContain)
                    }
                    FormLine(title: "Model 等于") {
                        TextField("精确模型名", text: $modelEquals)
                    }
                }
                SheetErrorText(submission.errorText)
            }
        }
    }

    private var canonicalRule: FeatureRule? {
        BuiltInFeatureRules.canonicalRule(id: rule.id)
    }

    private var candidateModels: [String] {
        // 钉住了入口就只给这个入口能接的模型(自带映射 + 已获取的模型目录);
        // 否则给所有入口显式映射声明的模型。
        FeatureRouteCandidates.models(
            config: model.config,
            endpointID: endpointID.isEmpty ? nil : endpointID
        )
    }

    @ViewBuilder
    private func readOnlyMatchRows(_ match: FeatureMatch) -> some View {
        if let value = match.requestKind {
            readOnlyMatchRow(title: "请求类型", value: "严格 \(value.displayName) 子请求形状")
        }
        if let value = match.toolTypePrefix {
            readOnlyMatchRow(title: "Tool prefix", value: value)
        }
        if let value = match.systemContains {
            readOnlyMatchRow(title: "System 包含", value: value)
        }
        if let value = match.messagesContain {
            readOnlyMatchRow(title: "Messages 包含", value: value)
        }
        if let value = match.modelEquals {
            readOnlyMatchRow(title: "Model 等于", value: value)
        }
    }

    private func readOnlyMatchRow(title: String, value: String) -> some View {
        FormLine(title: title) {
            Text(value)
                .font(.callout.monospaced())
                .textSelection(.enabled)
                .foregroundStyle(.secondary)
        }
    }

    private func submit() {
        guard submission.begin() else { return }
        Task {
            do {
                try await model.updateFeatureRuleAndSave(
                    id: rule.id,
                    enabled: enabled,
                    model: targetModel,
                    effortOverride: ReasoningEffort(rawValue: effortChoice),
                    protocolOverride: ProviderProtocol(rawValue: protocolChoice),
                    endpointID: endpointID.isEmpty ? nil : endpointID,
                    toolTypePrefix: toolTypePrefix,
                    systemContains: systemContains,
                    messagesContain: messagesContain,
                    modelEquals: modelEquals
                )
                submission.succeed()
                onClose()
            } catch {
                submission.fail(error.localizedDescription)
            }
        }
    }
}
