import Foundation
import XCTest
import SumpterCore
@testable import SumpterApp

final class GrokNotificationHooksTests: XCTestCase {
    func testNotificationEventsCoverSharedUserFacingCategories() {
        XCTAssertEqual(
            GrokNotifyHookFile.notificationEvents,
            ["Notification", "Stop", "StopFailure", "StopCancelled", "SubagentStop"]
        )
        XCTAssertEqual(
            GrokNotifyHookFile.notificationMatcher,
            "permission_prompt|idle_prompt|task_complete"
        )
    }

    func testUsesGrokHomeForPathsIncludingSpaces() {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("sumpter-grok home-\(UUID().uuidString)", isDirectory: true)
        let environment = ["GROK_HOME": root.path]
        XCTAssertEqual(GrokNotificationHooks.resolvedHomePath(environment: environment), root.path)
        XCTAssertEqual(
            GrokNotificationHooks.resolvedHooksJSONPath(environment: environment),
            root.appendingPathComponent("hooks", isDirectory: true)
                .appendingPathComponent("sumpter-notify.json").path
        )
    }

    func testEmptyGrokHomeFallsBackToUserGrokDirectory() {
        let path = GrokNotificationHooks.resolvedHomePath(environment: ["GROK_HOME": ""])
        XCTAssertTrue(path.hasSuffix("/.grok"))
    }

    func testNotificationHookClientsCoverClaudeCodexAndGrok() {
        XCTAssertEqual(
            NotificationHookClient.allCases.map(\.title),
            ["Claude Code", "Codex CLI", "Grok Build"]
        )
    }

    func testRemovedByUserFlagRoundTrips() {
        let previous = GrokNotificationHooks.wasRemovedByUser()
        defer { GrokNotificationHooks.markRemovedByUser(previous) }
        GrokNotificationHooks.markRemovedByUser(true)
        XCTAssertTrue(GrokNotificationHooks.wasRemovedByUser())
        GrokNotificationHooks.markRemovedByUser(false)
        XCTAssertFalse(GrokNotificationHooks.wasRemovedByUser())
    }
}
