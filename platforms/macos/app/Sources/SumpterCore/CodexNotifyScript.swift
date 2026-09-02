import Foundation

/// Codex CLI 生命周期 Hook 的脚本模板。
///
/// Codex 会把生命周期 payload 通过 stdin 传给 command。脚本只负责把原始
/// payload 送到本机 Sumpter；事件名从 Codex payload 中读取，因此同一脚本
/// 可以安全复用于 PermissionRequest、Stop、SubagentStop 和 Interrupt。
public enum CodexNotifyScript {
    public static let fileName = "sumpter-codex-notify.zsh"

    public static func command(path: String) -> String {
        "/bin/zsh \(shellQuote(path))"
    }

    public static func content(port: Int, token: String) -> String {
        let endpoint = shellQuote(
            "http://127.0.0.1:\(port)/__notify?token=\(token)&clientKind=codex"
        )
        return """
        #!/bin/zsh
        payload="$(/bin/cat 2>/dev/null || true)"
        [[ -z "$payload" ]] && payload="{}"
        /usr/bin/curl -sS --max-time 2 -X POST \(endpoint) -H "Content-Type: application/json" --data-binary "$payload" >/dev/null 2>&1 || true
        print -r -- '{"continue":true}'
        exit 0

        """
    }

    private static func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
    }
}
