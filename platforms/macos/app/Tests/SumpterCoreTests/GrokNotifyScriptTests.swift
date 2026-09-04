import XCTest
@testable import SumpterCore

final class GrokNotifyScriptTests: XCTestCase {
    func testScriptForwardsPayloadAndDoesNotBlockStop() {
        let script = GrokNotifyScript.content(port: 8484, token: "tok-abc")
        XCTAssertTrue(script.hasPrefix("#!/bin/zsh"))
        XCTAssertTrue(script.contains(#"payload="$(/bin/cat 2>/dev/null || true)""#))
        XCTAssertTrue(script.contains(#"--data-binary "$payload""#))
        XCTAssertTrue(script.contains("clientKind=grok_build"))
        XCTAssertFalse(script.contains("event=stop"))
        XCTAssertTrue(script.contains("--max-time 2"))
        XCTAssertFalse(script.contains("continue"))
        XCTAssertFalse(script.contains("decision"))
        XCTAssertTrue(script.hasSuffix("exit 0\n"))
    }

    func testEndpointShellQuotesToken() {
        let script = GrokNotifyScript.content(port: 8484, token: "a'b")
        XCTAssertTrue(script.contains("'\"'\"'"))
        XCTAssertTrue(script.contains("token=a"))
        XCTAssertTrue(script.contains("b&clientKind=grok_build"))
    }

    func testHookCommandShellQuotesPath() {
        let command = GrokNotifyScript.command(path: "/tmp/Grok Hooks/sumpter-notify.sh")
        XCTAssertEqual(command, "/bin/zsh '/tmp/Grok Hooks/sumpter-notify.sh'")
    }

    func testHookFileOwnsNotificationEventsAndMatchers() throws {
        let root = GrokNotifyHookFile.root(command: "/bin/zsh /tmp/sumpter-notify.sh")
        XCTAssertTrue(GrokNotifyHookFile.containsCommand(matching: "sumpter-notify.sh", in: root))
        XCTAssertFalse(GrokNotifyHookFile.containsCommand(matching: "other-hook", in: root))
        let hooks = try XCTUnwrap(root["hooks"] as? [String: Any])
        XCTAssertEqual(
            Set(hooks.keys),
            Set(GrokNotifyHookFile.notificationEvents)
        )
        let notification = try XCTUnwrap(hooks["Notification"] as? [[String: Any]])
        XCTAssertEqual(
            notification.first?["matcher"] as? String,
            GrokNotifyHookFile.notificationMatcher
        )
        let data = try GrokNotifyHookFile.encodeRoot(root)
        XCTAssertFalse(String(data: data, encoding: .utf8)?.isEmpty ?? true)
    }
}
