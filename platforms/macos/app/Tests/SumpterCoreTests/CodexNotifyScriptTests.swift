import XCTest
@testable import SumpterCore

final class CodexNotifyScriptTests: XCTestCase {
    func testScriptForwardsPayloadAndAlwaysContinues() {
        let script = CodexNotifyScript.content(port: 8484, token: "tok-abc")
        XCTAssertTrue(script.hasPrefix("#!/bin/zsh"))
        XCTAssertTrue(script.contains(#"payload="$(/bin/cat 2>/dev/null || true)""#))
        XCTAssertTrue(script.contains(#"--data-binary "$payload""#))
        XCTAssertTrue(script.contains("clientKind=codex"))
        XCTAssertFalse(script.contains("event=stop"))
        XCTAssertTrue(script.contains("--max-time 2"))
        XCTAssertTrue(script.contains(#"print -r -- '{"continue":true}'"#))
        XCTAssertTrue(script.hasSuffix("exit 0\n"))
    }

    func testEndpointShellQuotesToken() {
        let script = CodexNotifyScript.content(port: 8484, token: "a'b")
        XCTAssertTrue(script.contains("'\"'\"'"))
        XCTAssertTrue(script.contains("token=a"))
        XCTAssertTrue(script.contains("b&clientKind=codex"))
    }

    func testHookCommandShellQuotesPath() {
        let command = CodexNotifyScript.command(path: "/tmp/Codex Hooks/sumpter-codex-notify.zsh")
        XCTAssertEqual(command, "/bin/zsh '/tmp/Codex Hooks/sumpter-codex-notify.zsh'")
    }
}
