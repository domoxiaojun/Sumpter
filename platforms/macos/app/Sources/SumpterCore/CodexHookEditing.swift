import Foundation

public enum CodexHookEditingError: Error, LocalizedError, Equatable {
    case invalidJSON
    case invalidRoot
    case invalidField(String)

    public var errorDescription: String? {
        switch self {
        case .invalidJSON:
            return "Codex hooks.json 不是合法 JSON，拒绝覆盖"
        case .invalidRoot:
            return "Codex hooks.json 根节点不是对象，拒绝覆盖"
        case .invalidField(let field):
            return "Codex hooks.json 字段格式无效：\(field)，拒绝覆盖"
        }
    }
}

/// Codex hooks.json 的最小安全编辑器。
///
/// 使用 Foundation JSON 对象是为了保留 Codex 未来新增的未知字段；编辑器
/// 只触碰指定事件中命令路径包含 marker 的 Sumpter 条目。
public enum CodexHookEditing {
    /// Sumpter 当前接入的 Codex 生命周期事件。普通工具/压缩/会话事件不
    /// 默认接入系统通知，避免每次工具调用都打断用户。
    public static let notificationEvents = [
        "PermissionRequest", "Stop", "SubagentStop", "Interrupt"
    ]

    /// Codex 设置页的条目标题。缺省会显示成「钩子 1」。
    public static let hookDisplayName = "Sumpter 通知"

    public static func loadRoot(data: Data) throws -> [String: Any] {
        guard let object = try? JSONSerialization.jsonObject(with: data),
              let root = object as? [String: Any] else {
            if (try? JSONSerialization.jsonObject(with: data)) == nil {
                throw CodexHookEditingError.invalidJSON
            }
            throw CodexHookEditingError.invalidRoot
        }
        return try validateRoot(root)
    }

    public static func encodeRoot(_ root: [String: Any]) throws -> Data {
        _ = try validateRoot(root)
        return try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
    }

    public static func containsCommand(matching marker: String, in root: [String: Any]) throws -> Bool {
        try containsCommand(matching: marker, event: "Stop", in: root)
    }

    public static func containsCommand(
        matching marker: String,
        event: String,
        in root: [String: Any]
    ) throws -> Bool {
        let hooks = try hooksObject(in: root, required: false)
        guard let hooks else { return false }
        let rows = try eventRows(in: hooks, key: event, required: false)
        guard let rows else { return false }
        return commands(in: rows).contains { $0.contains(marker) }
    }

    public static func upsertStopCommand(
        _ command: String,
        matching marker: String,
        into root: [String: Any]
    ) throws -> [String: Any] {
        try upsertCommand(command, event: "Stop", matching: marker, into: root)
    }

    public static func upsertCommand(
        _ command: String,
        event: String,
        matching marker: String,
        into root: [String: Any]
    ) throws -> [String: Any] {
        var result = try validateRoot(root)
        var hooks = try hooksObject(in: result, required: false) ?? [:]
        var rows = try eventRows(in: hooks, key: event, required: false) ?? []
        rows = removingCommands(matching: marker, from: rows)
        rows.append([
            "name": hookDisplayName,
            "hooks": [[
                "type": "command",
                "name": hookDisplayName,
                "command": command,
                "async": true,
                "timeout": 3,
                "statusMessage": hookDisplayName
            ]]
        ])
        hooks[event] = rows
        result["hooks"] = hooks
        if result["description"] == nil {
            result["description"] = hookDisplayName
        }
        return result
    }

    public static func upsertCommands(
        _ commands: [(event: String, command: String)],
        matching marker: String,
        into root: [String: Any]
    ) throws -> [String: Any] {
        try commands.reduce(root) { result, entry in
            try upsertCommand(entry.command, event: entry.event, matching: marker, into: result)
        }
    }

    public static func removingStopCommand(
        matching marker: String,
        from root: [String: Any]
    ) throws -> [String: Any] {
        try removingCommand(matching: marker, event: "Stop", from: root)
    }

    public static func removingCommand(
        matching marker: String,
        event: String,
        from root: [String: Any]
    ) throws -> [String: Any] {
        var result = try validateRoot(root)
        guard var hooks = try hooksObject(in: result, required: false),
              let rows = try eventRows(in: hooks, key: event, required: false) else {
            return result
        }
        let kept = removingCommands(matching: marker, from: rows)
        if kept.isEmpty {
            hooks.removeValue(forKey: event)
        } else {
            hooks[event] = kept
        }
        if hooks.isEmpty {
            result.removeValue(forKey: "hooks")
        } else {
            result["hooks"] = hooks
        }
        return result
    }

    public static func removingCommands(
        matching marker: String,
        events: [String],
        from root: [String: Any]
    ) throws -> [String: Any] {
        try events.reduce(root) { result, event in
            try removingCommand(matching: marker, event: event, from: result)
        }
    }

    public static func commands(in rows: [[String: Any]]) -> [String] {
        rows.flatMap { row in
            (row["hooks"] as? [[String: Any]] ?? []).compactMap { hook in
                guard hook["type"] as? String == "command" else { return nil }
                return hook["command"] as? String
            }
        }
    }

    private static func removingCommands(matching marker: String, from rows: [[String: Any]]) -> [[String: Any]] {
        rows.compactMap { row in
            guard let handlers = row["hooks"] as? [[String: Any]] else { return row }
            let kept = handlers.filter { handler in
                guard handler["type"] as? String == "command",
                      let command = handler["command"] as? String else { return true }
                return !command.contains(marker)
            }
            guard !kept.isEmpty else { return nil }
            var copy = row
            copy["hooks"] = kept
            return copy
        }
    }

    private static func validateRoot(_ root: [String: Any]) throws -> [String: Any] {
        _ = try hooksObject(in: root, required: false)
        return root
    }

    private static func hooksObject(
        in root: [String: Any],
        required: Bool
    ) throws -> [String: Any]? {
        if let description = root["description"], !(description is String || description is NSNull) {
            throw CodexHookEditingError.invalidField("description")
        }
        guard let value = root["hooks"] else {
            if required { throw CodexHookEditingError.invalidField("hooks") }
            return nil
        }
        guard let hooks = value as? [String: Any] else {
            throw CodexHookEditingError.invalidField("hooks")
        }
        for event in [
            "PreToolUse", "PermissionRequest", "PostToolUse", "PreCompact",
            "PostCompact", "SessionStart", "SessionEnd", "UserPromptSubmit",
            "SubagentStart", "SubagentStop", "Stop", "Interrupt"
        ] {
            _ = try eventRows(in: hooks, key: event, required: false)
        }
        if let state = hooks["state"], !(state is [String: Any]) {
            throw CodexHookEditingError.invalidField("hooks.state")
        }
        if let state = hooks["state"] as? [String: Any] {
            for (key, value) in state {
                guard let entry = value as? [String: Any] else {
                    throw CodexHookEditingError.invalidField("hooks.state.\(key)")
                }
                if let enabled = entry["enabled"], !(enabled is Bool || enabled is NSNull) {
                    throw CodexHookEditingError.invalidField("hooks.state.\(key).enabled")
                }
                if let trustedHash = entry["trusted_hash"],
                   !(trustedHash is String || trustedHash is NSNull) {
                    throw CodexHookEditingError.invalidField("hooks.state.\(key).trusted_hash")
                }
                if let trustedHash = entry["trustedHash"],
                   !(trustedHash is String || trustedHash is NSNull) {
                    throw CodexHookEditingError.invalidField("hooks.state.\(key).trustedHash")
                }
            }
        }
        return hooks
    }

    private static func eventRows(
        in hooks: [String: Any],
        key: String,
        required: Bool
    ) throws -> [[String: Any]]? {
        guard let value = hooks[key] else {
            if required { throw CodexHookEditingError.invalidField("hooks.\(key)") }
            return nil
        }
        guard let rows = value as? [[String: Any]] else {
            throw CodexHookEditingError.invalidField("hooks.\(key)")
        }
        for (index, row) in rows.enumerated() {
            if let matcher = row["matcher"], !(matcher is String || matcher is NSNull) {
                throw CodexHookEditingError.invalidField("hooks.\(key)[\(index)].matcher")
            }
            if let name = row["name"], !(name is String || name is NSNull) {
                throw CodexHookEditingError.invalidField("hooks.\(key)[\(index)].name")
            }
            guard let handlers = row["hooks"] else { continue }
            guard let handlers = handlers as? [[String: Any]] else {
                throw CodexHookEditingError.invalidField("hooks.\(key)[\(index)].hooks")
            }
            for (handlerIndex, handler) in handlers.enumerated() {
                try validateHandler(
                    handler,
                    path: "hooks.\(key)[\(index)].hooks[\(handlerIndex)]"
                )
            }
        }
        return rows
    }

    private static func validateHandler(_ handler: [String: Any], path: String) throws {
        guard let rawType = handler["type"] else {
            throw CodexHookEditingError.invalidField("\(path).type")
        }
        guard let type = rawType as? String else {
            throw CodexHookEditingError.invalidField("\(path).type")
        }
        switch type {
        case "command":
            try requireString(handler, key: "command", path: path)
            try optionalString(handler, key: "commandWindows", path: path)
            try optionalString(handler, key: "command_windows", path: path)
            try optionalUnsignedInteger(handler, key: "timeout", path: path)
            try optionalBool(handler, key: "async", path: path)
            try optionalString(handler, key: "statusMessage", path: path)
            try optionalString(handler, key: "name", path: path)
            try optionalUnsignedInteger(handler, key: "additionalContextLimit", path: path)
        case "mcp_tool":
            try requireString(handler, key: "server", path: path)
            try requireString(handler, key: "tool", path: path)
            if let input = handler["input"], !(input is [String: Any]) {
                throw CodexHookEditingError.invalidField("\(path).input")
            }
            try optionalUnsignedInteger(handler, key: "timeout", path: path)
            try optionalString(handler, key: "statusMessage", path: path)
        case "prompt", "agent":
            break
        default:
            // Keep a future/user-defined handler untouched.  The value has
            // the correct JSON type; rejecting it here would make an app
            // upgrade destructive for fields this editor does not own.
            break
        }
    }

    private static func requireString(
        _ object: [String: Any],
        key: String,
        path: String
    ) throws {
        guard object[key] is String else {
            throw CodexHookEditingError.invalidField("\(path).\(key)")
        }
    }

    private static func optionalString(
        _ object: [String: Any],
        key: String,
        path: String
    ) throws {
        guard let value = object[key] else { return }
        guard value is String || value is NSNull else {
            throw CodexHookEditingError.invalidField("\(path).\(key)")
        }
    }

    private static func optionalBool(
        _ object: [String: Any],
        key: String,
        path: String
    ) throws {
        guard let value = object[key] else { return }
        guard value is Bool || value is NSNull else {
            throw CodexHookEditingError.invalidField("\(path).\(key)")
        }
    }

    private static func optionalUnsignedInteger(
        _ object: [String: Any],
        key: String,
        path: String
    ) throws {
        guard let value = object[key], !(value is NSNull) else { return }
        guard let number = value as? NSNumber,
              !(value is Bool),
              number.doubleValue.isFinite,
              number.doubleValue >= 0,
              number.doubleValue.rounded() == number.doubleValue,
              number.doubleValue <= Double(UInt64.max) else {
            throw CodexHookEditingError.invalidField("\(path).\(key)")
        }
    }
}
