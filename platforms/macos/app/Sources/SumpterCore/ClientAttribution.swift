import Foundation

public struct ClientAttribution: Codable, Equatable, Sendable {
    public struct Evidence: Codable, Equatable, Sendable {
        public let field: String
        public let source: String
        public let rule: String
        public let value: String
    }
    public let version: Int
    public let evidence: [Evidence]
    public let conflicts: [String]
    public let issues: [String]
}

extension RuntimeEventPresentation {
    public static func observedRequestKind(_ value: String?) -> String {
        guard let value, !value.isEmpty, value != "unrecorded" else { return "未记录" }
        return ["turn": "生成回合", "prewarm": "预热", "compaction": "上下文压缩", "session_title": "会话标题",
                "memory": "记忆任务", "review": "审查", "websearch": "搜索", "webfetch": "网页抓取",
                "classifier": "安全分类", "token_count": "Token 计数", "image_generation": "图片生成",
                "image_edit": "图片编辑", "alpha_search": "独立搜索", "realtime": "实时会话", "unknown": "未识别"][value] ?? "未识别"
    }

    public static func clientVariant(_ value: String?) -> String {
        guard let value, !value.isEmpty, value != "unrecorded" else { return "未记录" }
        return ["cli": "CLI", "tui": "TUI", "desktop": "桌面", "ide": "IDE", "vscode": "VS Code",
                "sdk": "SDK", "web": "网页", "backend": "服务端", "exec": "非交互执行", "unknown": "未识别"][value] ?? value
    }

    public static func agentRole(_ value: String?) -> String {
        guard let value, !value.isEmpty, value != "unrecorded" else { return "未记录" }
        return ["root": "主代理", "subagent": "子代理", "guardian": "Guardian", "review": "审查", "memory": "记忆任务",
                "title": "标题任务", "automation": "自动任务", "system": "系统", "ambient": "后台任务", "unknown": "未识别"][value] ?? value
    }

    public static func clientAttribution(_ attribution: ClientAttribution?) -> String {
        guard let attribution else { return "未记录" }
        let fields = ["clientKind": "客户端", "clientVariant": "形态", "agentRole": "角色", "requestKind": "请求类型"]
        let sources = ["declared": "客户端声明", "native": "原生标记", "endpoint": "专用接口", "user_agent": "产品 UA", "fingerprint": "固定指纹"]
        var lines = attribution.evidence.map { e in
            "\(fields[e.field] ?? e.field)：\(sources[e.source] ?? e.source) / \(e.rule) → \(e.value)"
        }
        if !attribution.conflicts.isEmpty {
            lines.append("信号冲突：" + attribution.conflicts.map { fields[$0] ?? $0 }.joined(separator: "、"))
        }
        if !attribution.issues.isEmpty { lines.append("已忽略无效字段：" + attribution.issues.joined(separator: "、")) }
        return lines.isEmpty ? "未识别（缺少有效依据）" : lines.joined(separator: "；")
    }
}
