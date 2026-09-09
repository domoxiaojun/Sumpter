import SumpterCore
import SwiftUI

struct RuntimeEventInspector: View {
    let event: RuntimeEvent
    let chain: [RuntimeEvent]
    @Binding var expanded: Set<String>

    private let groups = [("routing", "路由与重试"), ("usage", "用量与缓存"), ("identity", "会话与代理"),
                          ("response", "请求与响应"), ("tools", "工具与通知"), ("advanced", "高级诊断")]
    private var notify: Bool { event.kind == "notify" }
    private var usage: ResponseUsage? { event.observedUsage }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("选中事件").font(.headline)
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                InfoRow(title: "时间 / 类型", value: "\(RuntimeEventDisplay.dateTime(event.timestamp)) · \(RuntimeEventDisplay.kind(event.kind))")
                InfoRow(title: "客户端 / 项目", value: RuntimeEventDisplay.requestSummary(event))
                if notify { InfoRow(title: "Hook 类型", value: event.hookEvent ?? "—") }
                if !notify {
                    InfoRow(title: "客户端模型 / 入口", value: "\(event.clientModel ?? "—") · \(RuntimeEventDisplay.endpoint(event))")
                    InfoRow(title: event.kind == "upstream" ? "上游 HTTP" : "客户端 HTTP", value: RuntimeEventDisplay.httpStatus(event))
                        .foregroundStyle(RuntimeEventDisplay.httpStatusColor(event))
                    InfoRow(title: "最终结果", value: RuntimeEventDisplay.outcome(event))
                        .foregroundStyle(RuntimeEventDisplay.statusColor(event))
                    InfoRow(title: "TTFB / 总耗时", value: "\(event.ttfbMS.map { RuntimeEventPresentation.durationDisplay($0) } ?? "—") / \(RuntimeEventPresentation.durationDisplay(event.durationMS))")
                    InfoRow(title: "缓存 / 用量", value: "\(event.cacheReadLabel) · \(event.usageSummaryLabel)")
                }
            }
            let message = RuntimeEventDisplay.friendlyMessage(event)
            if !message.isEmpty || event.isFailed || event.failover {
                Text((event.failover ? "重试 / 故障转移 · " : "") + (message.isEmpty ? failureSummary : message))
                    .font(.callout).foregroundStyle(event.isFailed ? Color.red : Color.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            ForEach(groups.filter { !notify || ["identity", "tools", "advanced"].contains($0.0) }, id: \.0) { key, title in
                Divider()
                FullRowDisclosure(isExpanded: binding(key), label: {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(title).font(.callout.weight(.semibold))
                        Text(summary(key)).font(.caption).foregroundStyle(.secondary)
                    }
                }) {
                    if event.detailsOmitted == true {
                        Text("正在加载详情…").font(.caption).foregroundStyle(.secondary)
                    } else {
                        content(key)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    private func binding(_ key: String) -> Binding<Bool> {
        let id = "\(event.id):\(key)"
        return Binding(get: { expanded.contains(id) }, set: { value in
            if value { expanded.insert(id) } else { expanded.remove(id) }
        })
    }

    private var failureSummary: String {
        event.failureKind.map(RuntimeEventPresentation.failureKindDisplay) ?? (event.isFailed ? "响应失败，原因未报告" : "")
    }

    private func summary(_ key: String) -> String {
        switch key {
        case "routing": "\(RuntimeEventDisplay.endpoint(event)) · \(chain.filter { $0.kind == "upstream" }.count) 次上游尝试"
        case "usage": event.cacheReadLabel
        case "identity": event.agentSummaryLabel ?? "会话、父子关系与归因来源"
        case "response": "\(event.requestMethod ?? "—") · \(RuntimeEventDisplay.httpStatus(event))"
        case "tools": event.toolCalls.map { "\($0.count) 种工具" } ?? event.hookEvent ?? "按需查看"
        default: "元数据、流观测与原始事件"
        }
    }

    @ViewBuilder private func rows(_ fields: [(String, String?)]) -> some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
            ForEach(Array(fields.enumerated()), id: \.offset) { _, item in
                InfoRow(title: item.0, value: item.1 ?? "—", copyable: item.1 != nil)
            }
        }
    }

    @ViewBuilder private func content(_ key: String) -> some View {
        switch key {
        case "routing":
            rows([("客户端模型", event.clientModel), ("逻辑模型", event.effectiveModel), ("上游模型", event.upstreamModel),
                  ("入口", event.endpointName), ("模型组", event.modelGroupName ?? event.modelGroupID),
                  ("命中规则", event.featureRuleID), ("用途", RuntimeEventDisplay.purpose(event))])
        case "usage":
            rows([("缓存读占比", event.cacheReadTokenRatio.map { $0.formatted(.percent.precision(.fractionLength(1))) }), ("缓存读状态", event.cacheReadLabel), ("缓存证据", event.cacheRead?.reasonLabel),
                  ("缓存读计数", ["confirmed": "已确认", "provisional": "暂定，可能增加", "unknown": "未知"][event.cacheRead?.finality ?? "unknown"]),
                  ("输入 token", usage?.inputTokens.map(String.init)), ("输出 token", usage?.outputTokens.map(String.init)),
                  ("缓存读 token", usage?.cacheReadInputTokens.map(String.init)), ("缓存写 token", usage?.cacheCreationInputTokens.map(String.init)),
                  ("推理 token（输出子集）", usage?.reasoningTokens.map(String.init))])
        case "identity":
            rows([("代理", event.agentSummaryLabel), ("客户端形态", event.clientVariant), ("请求 ID", event.requestID),
                  ("会话 ID", event.sessionID), ("会话来源", event.sessionSource), ("线程 ID", event.codexMetadata?.threadID),
                  ("回合 ID", event.codexMetadata?.turnID), ("父线程", event.parentThreadID ?? event.codexMetadata?.parentThreadID),
                  ("父回合", event.parentTurnID ?? event.codexMetadata?.parentTurnID), ("根回合", event.rootTurnID ?? event.codexMetadata?.rootTurnID),
                  ("Grok agent ID", event.grokMetadata?.agentID), ("项目来源", event.projectSource), ("工作区", event.clientDeclared?.workspace)])
        case "response":
            rows([("方法", event.requestMethod), ("路径", event.requestPath),
                  ("协议", RuntimeEventPresentation.protocolPath(sourceFormat: event.sourceFormat, targetFormat: event.targetFormat, routeMode: event.routeMode)),
                  ("本层 HTTP", RuntimeEventDisplay.httpStatus(event)), ("直接上游 HTTP", event.upstreamStatusCode.map { "HTTP \($0)" }),
                  ("最终结果", RuntimeEventDisplay.outcome(event)), ("失败类型 / 阶段", failureSummary),
                  ("停止原因", event.streamTrace?.stopReason), ("超时阈值", event.timeoutMS.map { RuntimeEventPresentation.durationDisplay($0) }),
                  ("错误详情", event.failureDetail)])
        case "tools":
            rows([("工具名称（去重）", event.toolCalls?.joined(separator: "、")),
                  ("工具观测截断", event.streamTrace?.toolCallsTruncated == true ? "是" : "否"),
                  ("Hook 类型", event.hookEvent), ("通知文案", notify ? event.message : nil)])
        default:
            rows([("上游追踪 ID", event.upstreamRequestID), ("上游主机", event.upstreamHost), ("入口 ID", event.endpointID), ("事件 ID", event.id)])
            Button("复制完整事件 JSON") { _ = PasteboardCopy.write(eventJSON) }
            ScrollView { Text(eventJSON).font(.caption.monospaced()).frame(maxWidth: .infinity, alignment: .leading) }
                .frame(maxHeight: 480)
        }
    }

    private var eventJSON: String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return (try? encoder.encode(event)).flatMap { String(data: $0, encoding: .utf8) } ?? "无法编码事件"
    }
}
