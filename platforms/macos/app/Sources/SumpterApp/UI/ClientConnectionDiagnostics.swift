import SumpterCore
import SwiftUI

private struct ClientDiagnostic: Decodable, Identifiable {
    struct Source: Decodable { let source: String; let baseURL: String; let matchesProxyOrigin: Bool; let explicitOptIn: Bool? }
    let client: String
    let status: String
    let wrapperVersion: String
    let sources: [Source]
    let issues: [String]
    let note: String
    var id: String { client }
}

struct ClientConnectionDiagnostics: View {
    @ObservedObject var model: AppModel
    @State private var rows: [ClientDiagnostic] = []
    @State private var busy = false
    @State private var error: String?
    private let clients = [("Claude Code", "claude_code"), ("Codex", "codex"), ("Gemini CLI", "gemini_cli"), ("pi", "pi"), ("Grok Build", "grok_build")]

    var body: some View {
        SectionPanel(title: "客户端连接诊断", hint: "未见请求表示未知，不能据此判断客户端绕过代理。") {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(clients, id: \.1) { label, kind in
                    HStack {
                        Text(label).frame(width: 100, alignment: .leading)
                        if let event = model.runtime.recentEvents.first(where: { $0.kind == "client" && $0.clientKind?.rawValue == kind }) {
                            Text("已观察到请求 · \(RuntimeEventDisplay.dateTime(event.timestamp))")
                        } else { Text("最近窗口未见请求").foregroundStyle(.secondary) }
                    }.font(.caption)
                }
                Button(busy ? "检查中…" : "检查本机可见配置") {
                    busy = true; error = nil
                    Task { @MainActor in
                        defer { busy = false }
                        do {
                            let output = try await UnifiedAttributionInstaller.diagnose(proxyURL: "http://127.0.0.1:\(model.config.listener.port)")
                            rows = try JSONDecoder().decode([ClientDiagnostic].self, from: Data(output.utf8))
                        } catch { self.error = error.localizedDescription }
                    }
                }.disabled(busy)
                if let error { Text(error).font(.caption).foregroundStyle(.red) }
                ForEach(rows) { row in
                    DisclosureGroup("\(row.client) · 包装器 \(row.wrapperVersion)") {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("安装状态：\(statusLabel(row.status))")
                            ForEach(Array(row.sources.enumerated()), id: \.offset) { _, source in
                                Text("\(source.source)：\(source.baseURL)")
                                Text(source.matchesProxyOrigin ? "与指定代理地址同源" : "与指定代理地址不同源，需核对实际配置")
                                    .foregroundStyle(.secondary)
                                if let marked = source.explicitOptIn { Text(marked ? "已显式启用 Sumpter 归因" : "未设置 Sumpter 归因标记") }
                            }
                            if row.sources.isEmpty { Text("未发现可确认的地址，状态未知。") }
                            ForEach(row.issues, id: \.self) { Text($0).foregroundStyle(.secondary) }
                            Text(row.note).foregroundStyle(.secondary)
                        }.font(.caption).textSelection(.enabled).padding(.vertical, 8)
                    }
                }
                Text("此检查不读取鉴权存储、不执行模型请求、不修改配置。Finder 启动的 App 无法获知终端当前的环境变量，终端场景请运行 doctor 命令。")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func statusLabel(_ status: String) -> String {
        ["installed": "已安装", "absent": "未安装", "legacy": "旧版", "broken": "不完整", "outdated": "需要更新"][status] ?? "未知"
    }
}
