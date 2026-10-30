import SumpterCore
import SwiftUI

struct RequestPolicySheet: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var draft: RequestPolicy
    @State private var endpointID = ""
    @State private var modelID = "*"
    @State private var protocolName = "all"
    @State private var error: String?
    @State private var saving = false
    private let original: RequestPolicy

    init(model: AppModel) {
        self.model = model
        original = model.config.requestPolicy
        _draft = State(initialValue: model.config.requestPolicy)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("全客户端请求策略").font(.title2.bold())
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("适用于 HTTP/SSE。并发名额保持到上游响应结束；重试也计入每分钟额度。WebSocket 不按连接数等同为请求数。")
                        .font(.caption).foregroundStyle(.secondary)
                    GroupBox("全局请求限制") { limits($draft.global) }
                    number("一次请求最多尝试（0 = 沿用原有重试限制）", $draft.maxTotalAttempts, 0...10_000)
                    Button("填入交互预设") {
                        draft.maxTotalAttempts = 6; draft.global.maxConcurrent = 8
                        draft.global.maxQueued = 16; draft.global.queueTimeoutSeconds = 15
                    }
                    GroupBox("Responses 流保护") {
                        VStack(alignment: .leading, spacing: 12) {
                            Picker("流式策略", selection: $draft.streamProtection) {
                                Text("完整缓冲：支持中途断流恢复").tag("complete")
                                Text("前奏保护：更早开始输出").tag("prelude")
                            }
                            Text("仅在原有流错误重试开启时生效。前奏模式提交业务输出后不再透明重放；达到缓冲额度时恢复透传，并在请求详情记录原因。")
                                .font(.caption).foregroundStyle(.secondary)
                            number("单流缓冲（MiB）", $draft.streamBufferMiB, 1...256)
                            number("所有流总缓冲（MiB）", $draft.maxBufferedMiB, 1...4096)
                            number("单流响应块上限", $draft.streamBufferChunks, 16...65_536)
                            number("缓冲时限（秒，0 = 不限）", $draft.streamBufferSeconds, 0...3600)
                        }.padding(8)
                    }
                    GroupBox("入口限制与兼容声明") {
                        VStack(alignment: .leading, spacing: 12) {
                            Picker("入口", selection: $endpointID) {
                                Text("选择入口").tag("")
                                ForEach(model.config.endpoints, id: \.id) { endpoint in Text(endpoint.name.isEmpty ? endpoint.id : endpoint.name).tag(endpoint.id) }
                            }
                            if !endpointID.isEmpty {
                                limits(Binding(get: { draft.endpoints[endpointID]?.limits ?? RequestLimits() }, set: { draft.endpoints[endpointID, default: EndpointRequestPolicy()].limits = $0 }))
                                TextField("上游模型（精确 ID 或 *）", text: $modelID)
                                Picker("兼容声明适用协议", selection: $protocolName) {
                                    Text("所有协议的默认声明").tag("all")
                                    Text("Anthropic Messages").tag("anthropic")
                                    Text("Chat Completions").tag("openai")
                                    Text("Responses").tag("openai-responses")
                                    Text("Gemini").tag("gemini")
                                }
                                Text("未知保持原样；不支持时跳过该入口，不静默删除参数。所有候选都不支持则拒绝请求。")
                                    .font(.caption).foregroundStyle(.secondary)
                                compatibility("严格工具", \.strictTools)
                                compatibility("语法工具", \.grammarTools)
                                compatibility("Developer 角色", \.developerRole)
                                compatibility("结构化输出", \.structuredOutput)
                            }
                        }.padding(8)
                    }
                }
            }
            if let error { Text(error).foregroundStyle(.red).font(.caption) }
            HStack {
                Spacer()
                Button("取消") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(saving ? "保存中…" : "保存请求策略") {
                    saving = true
                    Task { @MainActor in
                        defer { saving = false }
                        do {
                            try await model.mutateConfig { config in
                                guard config.requestPolicy == original else { throw AppModelError.invalidInput("请求策略已被其它操作修改，请关闭后重新打开。") }
                                config.requestPolicy = draft
                            }
                            dismiss()
                        } catch { self.error = error.localizedDescription }
                    }
                }.buttonStyle(.borderedProminent).disabled(saving)
            }
        }.padding(24).frame(width: 690, height: 680)
    }

    private func number(_ title: String, _ value: Binding<Int>, _ range: ClosedRange<Int>) -> some View {
        HStack {
            Text(title)
            Spacer()
            TextField(title, value: Binding(get: { value.wrappedValue }, set: { value.wrappedValue = min(range.upperBound, max(range.lowerBound, $0)) }), format: .number)
                .textFieldStyle(.roundedBorder).frame(width: 100).accessibilityLabel(title)
        }
    }

    private func limits(_ value: Binding<RequestLimits>) -> some View {
        VStack(spacing: 12) {
            number("并发上限（0 = 不限）", value.maxConcurrent, 0...10_000)
            number("排队数量（0 = 不排队）", value.maxQueued, 0...10_000)
            number("排队时限（秒）", value.queueTimeoutSeconds, 1...300)
            number("每分钟上游尝试（0 = 不限）", value.requestsPerMinute, 0...1_000_000)
        }.padding(8)
    }

    private func compatibility(_ title: String, _ key: WritableKeyPath<CompatibilityProfile, Bool?>) -> some View {
        let id = modelID.trimmingCharacters(in: .whitespacesAndNewlines)
        return Picker(title, selection: Binding<String>(get: {
            let profiles = protocolName == "all" ? draft.endpoints[endpointID]?.models : draft.endpoints[endpointID]?.protocolModels[protocolName]
            guard let v = profiles?[id]?[keyPath: key] else { return "unknown" }
            return v ? "true" : "false"
        }, set: { value in
            var endpoint = draft.endpoints[endpointID] ?? EndpointRequestPolicy()
            var profiles = (protocolName == "all" ? endpoint.models : endpoint.protocolModels[protocolName]) ?? [:]
            var profile = profiles[id] ?? CompatibilityProfile()
            profile[keyPath: key] = value == "unknown" ? nil : value == "true"
            profiles[id] = profile
            if protocolName == "all" { endpoint.models = profiles } else { endpoint.protocolModels[protocolName] = profiles }
            draft.endpoints[endpointID] = endpoint
        })) {
            Text("未知 / 保持原样").tag("unknown"); Text("支持").tag("true"); Text("不支持").tag("false")
        }.disabled(id.isEmpty)
    }
}
