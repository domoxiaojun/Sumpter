import Foundation

/// Claude Code hook 脚本模板(纯逻辑,可单测)。
///
/// v2:把 Claude Code 经 stdin 传入的完整 hook payload 原样转发给 sumpterd
/// (真实提示文本 message、cwd、session_id 都在里面),事件名走查询参数 `event=`
/// (sumpterd 的 token 校验按 `&` 拆参,多余参数不影响)。富化逻辑集中在
/// sumpterd 侧(engine.rs handle_notify)。旧版脚本只发事件名,通知恒简陋。
public enum ClaudeNotifyScript {
    public static func content(port: Int, token: String) -> String {
        """
        #!/bin/zsh
        event="${1:-notification}"
        payload="$(cat 2>/dev/null || true)"
        [[ -z "$payload" ]] && payload="{}"
        /usr/bin/curl -sS --max-time 2 -X POST "http://127.0.0.1:\(port)/__notify?token=\(token)&event=$event" -H "Content-Type: application/json" --data-binary "$payload" >/dev/null 2>&1
        exit 0
        """
    }
}
