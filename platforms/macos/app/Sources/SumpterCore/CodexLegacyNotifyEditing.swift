import Foundation

public enum CodexLegacyNotifyEditStatus: Equatable, Sendable {
    case absent
    case removed
    case conflict
}

public struct CodexLegacyNotifyEditResult: Equatable, Sendable {
    public let text: String
    public let status: CodexLegacyNotifyEditStatus

    public init(text: String, status: CodexLegacyNotifyEditStatus) {
        self.text = text
        self.status = status
    }
}

/// 只识别当前 Codex 单行 legacy notify 形状，避免用不完整 TOML 解析器
/// 擅自改写用户自定义命令或多行配置。
public enum CodexLegacyNotifyEditing {
    public static func removeKnownNotify(from text: String) -> CodexLegacyNotifyEditResult {
        let pattern = #"(?m)^[\t ]*notify[\t ]*=[\t ]*(\[[^\r\n]*\])[\t ]*(?:#[^\r\n]*)?(?:\r?\n|$)"#
        guard let regex = try? NSRegularExpression(pattern: pattern) else {
            return CodexLegacyNotifyEditResult(text: text, status: .conflict)
        }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        let matches = regex.matches(in: text, range: range)
        guard let match = matches.first else {
            // A notify assignment that spans multiple lines (or otherwise
            // falls outside the deliberately narrow JSON-compatible shape)
            // is a conflict, not an absent setting.  Never silently rewrite
            // a custom TOML value we cannot prove is the built-in command.
            let hasNotifyAssignment = text.split(whereSeparator: \.isNewline).contains {
                let line = $0.trimmingCharacters(in: .whitespacesAndNewlines)
                return line.hasPrefix("notify") && line.dropFirst(6).first.map { $0 == "=" || $0.isWhitespace } == true
            }
            return CodexLegacyNotifyEditResult(
                text: text,
                status: hasNotifyAssignment ? .conflict : .absent
            )
        }
        guard matches.count == 1,
              let valueRange = Range(match.range(at: 1), in: text) else {
            return CodexLegacyNotifyEditResult(text: text, status: .conflict)
        }
        guard let fullRange = Range(match.range, in: text) else {
            return CodexLegacyNotifyEditResult(text: text, status: .conflict)
        }
        // `notify` is a top-level legacy setting.  Once TOML enters a table,
        // a similarly shaped key belongs to that table and must not be
        // removed by this migration.
        let hasTableHeader = text[..<fullRange.lowerBound].split(whereSeparator: \.isNewline).contains {
            let line = $0.trimmingCharacters(in: .whitespacesAndNewlines)
            return line.hasPrefix("[") && line.hasSuffix("]")
        }
        guard !hasTableHeader else {
            return CodexLegacyNotifyEditResult(text: text, status: .conflict)
        }
        let value = String(text[valueRange])
        guard let data = value.data(using: .utf8),
              let args = try? JSONSerialization.jsonObject(with: data) as? [String],
              args.count == 2,
              args[0].contains("SkyComputerUseClient"),
              args[1] == "turn-ended" else {
            return CodexLegacyNotifyEditResult(text: text, status: .conflict)
        }
        var result = text
        result.removeSubrange(fullRange)
        return CodexLegacyNotifyEditResult(text: result, status: .removed)
    }
}
