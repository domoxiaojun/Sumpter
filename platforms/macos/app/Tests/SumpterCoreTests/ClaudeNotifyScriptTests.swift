import XCTest
@testable import SumpterCore

/// hook 脚本模板:v2 必须转发 stdin 的完整 hook payload,事件名走 event= 查询参数。
/// 这是通知富化链路的入口——脚本不转发 payload,sumpterd 侧的富化无米下锅。
final class ClaudeNotifyScriptTests: XCTestCase {
    func testScriptForwardsStdinPayload() {
        let script = ClaudeNotifyScript.content(port: 8484, token: "tok-abc")
        // stdin 全量读入并作为请求体转发(--data-binary 不做换行改写)。
        XCTAssertTrue(script.contains(#"payload="$(cat 2>/dev/null || true)""#))
        XCTAssertTrue(script.contains("--data-binary \"$payload\""))
        // 空 stdin 兜底 {},老 sumpterd 收到也能解析。
        XCTAssertTrue(script.contains(#"[[ -z "$payload" ]] && payload="{}""#))
        // 事件名经查询参数传递;token 鉴权与端口内嵌。
        XCTAssertTrue(script.contains("http://127.0.0.1:8484/__notify?token=tok-abc&event=$event&clientKind=$clientKind"))
        XCTAssertTrue(script.contains(#"event="${1:-notification}""#))
        XCTAssertTrue(script.contains(#"clientKind="claude_code""#))
        XCTAssertTrue(script.contains("GROK_HOOK_EVENT"))
        XCTAssertTrue(script.contains(#"clientKind="grok_build""#))
        // 快速失败,不阻塞 Claude Code 的 hook 执行。
        XCTAssertTrue(script.contains("--max-time 2"))
        XCTAssertTrue(script.hasPrefix("#!/bin/zsh"))
        XCTAssertTrue(script.hasSuffix("exit 0"))
    }
}
