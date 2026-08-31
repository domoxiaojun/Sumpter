import Foundation

/// Claude Code `settings.json` 里 hooks 数组的纯编辑逻辑(可单测)。
///
/// hooks[event] 的形状是「匹配器行」数组,每行形如 `["matcher": ..., "hooks": [["type":"command","command": ...]]]`。
/// Sumpter只维护自己那条命令,必须保留用户在同一事件上已有的其它 hooks。
public enum ClaudeHookEditing {
    /// 该事件下是否已存在命令包含 `needle` 的条目。
    public static func containsCommand(matching needle: String, in value: Any?) -> Bool {
        commands(in: value).contains { $0.contains(needle) }
    }

    /// 移除所有命令包含 `needle` 的条目,保留其它;某匹配器行清空后整行丢弃。
    public static func removingCommands(matching needle: String, from value: Any?) -> [[String: Any]] {
        guard let rows = value as? [[String: Any]] else {
            return []
        }
        return rows.compactMap { row -> [String: Any]? in
            guard let commands = row["hooks"] as? [[String: Any]] else {
                return row
            }
            let kept = commands.filter { command in
                !(command["command"] as? String ?? "").contains(needle)
            }
            guard !kept.isEmpty else {
                return nil
            }
            var row = row
            row["hooks"] = kept
            return row
        }
    }

    /// 先移除匹配 `needle` 的旧命令,再追加一条 `command`,从而在保留用户其它 hooks 的前提下更新Sumpter那条。
    public static func upsertCommand(_ command: String, matching needle: String, into value: Any?) -> [[String: Any]] {
        var rows = removingCommands(matching: needle, from: value)
        rows.append(["hooks": [["type": "command", "command": command]]])
        return rows
    }

    /// 展平出该事件下所有命令字符串。
    public static func commands(in value: Any?) -> [String] {
        guard let rows = value as? [[String: Any]] else {
            return []
        }
        return rows.flatMap { row in
            (row["hooks"] as? [[String: Any]] ?? []).compactMap { $0["command"] as? String }
        }
    }
}
