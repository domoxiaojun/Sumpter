import XCTest
@testable import SumpterCore

final class CodexLegacyNotifyEditingTests: XCTestCase {
    func testRemovesKnownSkyComputerUseNotify() {
        let source = "model = \"gpt\"\nnotify = [\"/Applications/SkyComputerUseClient.app/Contents/MacOS/SkyComputerUseClient\", \"turn-ended\"]\n"
        let result = CodexLegacyNotifyEditing.removeKnownNotify(from: source)
        XCTAssertEqual(result.status, .removed)
        XCTAssertEqual(result.text, "model = \"gpt\"\n")
    }

    func testUnknownNotifyIsConflictAndUnchanged() {
        let source = "notify = [\"my-notifier\", \"turn-ended\"]\n"
        let result = CodexLegacyNotifyEditing.removeKnownNotify(from: source)
        XCTAssertEqual(result.status, .conflict)
        XCTAssertEqual(result.text, source)
    }

    func testMultilineNotifyIsConflictAndUnchanged() {
        let source = "notify = [\n  \"SkyComputerUseClient\",\n  \"turn-ended\"\n]\n"
        let result = CodexLegacyNotifyEditing.removeKnownNotify(from: source)
        XCTAssertEqual(result.status, .conflict)
        XCTAssertEqual(result.text, source)
    }

    func testAbsentNotifyIsNoop() {
        let source = "model = \"gpt\"\n"
        let result = CodexLegacyNotifyEditing.removeKnownNotify(from: source)
        XCTAssertEqual(result.status, .absent)
        XCTAssertEqual(result.text, source)
    }

    func testKnownNotifyInsideTomlTableIsConflictAndUnchanged() {
        let source = "[some_table]\nnotify = [\"SkyComputerUseClient\", \"turn-ended\"]\n"
        let result = CodexLegacyNotifyEditing.removeKnownNotify(from: source)
        XCTAssertEqual(result.status, .conflict)
        XCTAssertEqual(result.text, source)
    }

    func testKnownNotifyWithCommentIsRemoved() {
        let source = "notify = [\"SkyComputerUseClient\", \"turn-ended\"] # legacy\nmodel = \"gpt\"\n"
        let result = CodexLegacyNotifyEditing.removeKnownNotify(from: source)
        XCTAssertEqual(result.status, .removed)
        XCTAssertEqual(result.text, "model = \"gpt\"\n")
    }
}
