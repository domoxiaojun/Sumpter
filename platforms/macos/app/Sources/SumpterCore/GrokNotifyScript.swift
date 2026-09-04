import Foundation

/// Grok Build 生命周期 Hook 的脚本模板。
///
/// Grok 把事件 JSON 写到 stdin。脚本只负责原样转到本机 Sumpter；事件名从
/// payload 读取，因此同一脚本可复用于 Notification / Stop / StopFailure /
/// StopCancelled / SubagentStop。
///
/// Grok 的 `Stop` 是 gate：脚本必须 `exit 0` 且不打印 JSON，否则会挡住回合结束。
public enum GrokNotifyScript {
    public static let fileName = "sumpter-notify.sh"

    public static func command(path: String) -> String {
        "/bin/zsh \(shellQuote(path))"
    }

    public static func content(port: Int, token: String) -> String {
        let endpoint = shellQuote(
            "http://127.0.0.1:\(port)/__notify?token=\(token)&clientKind=grok_build"
        )
        return """
        #!/bin/zsh
        payload="$(/bin/cat 2>/dev/null || true)"
        [[ -z "$payload" ]] && payload="{}"
        /usr/bin/curl -sS --max-time 2 -X POST \(endpoint) -H "Content-Type: application/json" --data-binary "$payload" >/dev/null 2>&1 || true
        exit 0

        """
    }

    private static func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
    }
}

/// Sumpter 独占的 `~/.grok/hooks/sumpter-notify.json`。Grok 会 merge
/// `hooks/*.json`，所以只动这个文件，不改 `config.toml` 或用户其它 hook。
public enum GrokNotifyHookFile {
    public static let fileName = "sumpter-notify.json"

    public static let notificationEvents = [
        "Notification", "Stop", "StopFailure", "StopCancelled", "SubagentStop"
    ]

    public static let notificationMatcher = "permission_prompt|idle_prompt|task_complete"

    public static func root(command: String) -> [String: Any] {
        let handler: [String: Any] = [
            "type": "command",
            "command": command,
            "timeout": 5
        ]
        func group(matcher: String? = nil) -> [String: Any] {
            var row: [String: Any] = ["hooks": [handler]]
            if let matcher {
                row["matcher"] = matcher
            }
            return row
        }
        return [
            "hooks": [
                "Notification": [group(matcher: notificationMatcher)],
                "Stop": [group()],
                "StopFailure": [group()],
                "StopCancelled": [group()],
                "SubagentStop": [group()]
            ]
        ]
    }

    public static func encodeRoot(_ root: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
    }

    public static func containsCommand(matching marker: String, in root: [String: Any]) -> Bool {
        guard let hooks = root["hooks"] as? [String: Any] else { return false }
        return notificationEvents.contains { event in
            let rows = hooks[event] as? [[String: Any]] ?? []
            return rows.contains { row in
                let commands = row["hooks"] as? [[String: Any]] ?? []
                return commands.contains { command in
                    (command["command"] as? String ?? "").contains(marker)
                }
            }
        }
    }
}
