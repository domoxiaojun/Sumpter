import Foundation
import XCTest
import SumpterCore
@testable import SumpterApp

final class CodexNotificationHooksTests: XCTestCase {
    func testNotificationEventsCoverSharedUserFacingCategories() {
        XCTAssertEqual(
            CodexHookEditing.notificationEvents,
            ["PermissionRequest", "Stop", "SubagentStop", "Interrupt"]
        )
    }

    func testUsesCodexHomeForPathsIncludingSpaces() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-codex home-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let environment = ["CODEX_HOME": root.path]
        XCTAssertEqual(CodexNotificationHooks.resolvedHomePath(environment: environment), root.path)
        XCTAssertEqual(
            CodexNotificationHooks.resolvedHooksJSONPath(environment: environment),
            root.appendingPathComponent("hooks.json").path
        )
    }

    func testEmptyCodexHomeFallsBackToUserCodexDirectory() {
        let path = CodexNotificationHooks.resolvedHomePath(environment: ["CODEX_HOME": ""])
        XCTAssertTrue(path.hasSuffix("/.codex"))
    }
}
