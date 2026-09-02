import XCTest
@testable import SumpterApp

final class AdminClientNotificationTests: XCTestCase {
    func testCodexClientKindIsDecodedFromSSEWire() throws {
        let event = AdminWire.Event.parse(
            name: "notify",
            data: #"{"clientKind":"codex","title":"Codex CLI · 回合完成","message":"回合已完成","sound":null,"type":"notification","category":"turn_completed","priority":"normal","actionID":null,"sessionId":"s-1","cwd":"/tmp/project"}"#
        )
        guard case .notify(let clientKind, let title, _, _, _, _, _, _, _, _) = event else {
            return XCTFail("expected notify event")
        }
        XCTAssertEqual(clientKind, "codex")
        XCTAssertEqual(title, "Codex CLI · 回合完成")
    }

    func testLegacyNotifyWireDefaultsToClaudeCompatibility() throws {
        let event = AdminWire.Event.parse(
            name: "notify",
            data: #"{"title":"Claude Code · 回合结束","message":"已完成","sound":null,"type":"notification","category":"turn_completed","priority":"normal","actionID":null}"#
        )
        guard case .notify(let clientKind, _, _, _, _, _, _, _, _, _) = event else {
            return XCTFail("expected notify event")
        }
        XCTAssertNil(clientKind)
    }
}
