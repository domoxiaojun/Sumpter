import Foundation
import XCTest
@testable import SumpterCore

final class CodexHookEditingTests: XCTestCase {
    private let marker = CodexNotifyScript.fileName

    private func root(_ stop: Any? = nil) -> [String: Any] {
        var value: [String: Any] = ["description": "user config", "other": ["keep": true]]
        if let stop { value["hooks"] = ["SessionStart": [["hooks": [["type": "command", "command": "user-hook"]]]], "Stop": stop] }
        return value
    }

    func testUpsertPreservesUserHooksAndUnknownFields() throws {
        let existing: [[String: Any]] = [[
            "matcher": "done",
            "hooks": [["type": "command", "command": "/Users/me/own-hook"]]
        ]]
        let merged = try CodexHookEditing.upsertStopCommand(
            "/bin/zsh '/tmp/with space/\(marker)'", matching: marker, into: root(existing)
        )
        let hooks = try XCTUnwrap(merged["hooks"] as? [String: Any])
        XCTAssertNotNil(hooks["SessionStart"])
        let rows = try XCTUnwrap(hooks["Stop"] as? [[String: Any]])
        XCTAssertEqual(CodexHookEditing.commands(in: rows).count, 2)
        XCTAssertTrue(CodexHookEditing.commands(in: rows).contains("/Users/me/own-hook"))
        XCTAssertTrue(CodexHookEditing.commands(in: rows).contains { $0.contains(marker) })
        XCTAssertEqual((merged["description"] as? String), "user config")
    }

    func testUpsertReplacesStaleSumpterCommand() throws {
        let old: [[String: Any]] = [["hooks": [["type": "command", "command": "/old/\(marker)"]]]]
        let merged = try CodexHookEditing.upsertStopCommand("/new/\(marker)", matching: marker, into: root(old))
        let hooks = try XCTUnwrap(merged["hooks"] as? [String: Any])
        let rows = try XCTUnwrap(hooks["Stop"] as? [[String: Any]])
        XCTAssertEqual(CodexHookEditing.commands(in: rows).filter { $0.contains(marker) }, ["/new/\(marker)"])
    }

    func testUpsertMultipleNotificationEventsPreservesOtherCodexHooks() throws {
        let existing: [String: Any] = [
            "hooks": [
                "PermissionRequest": [["hooks": [["type": "command", "command": "/Users/me/policy"]]]],
                "Stop": [["hooks": [["type": "command", "command": "/Users/me/stop"]]]],
            ]
        ]
        let merged = try CodexHookEditing.upsertCommands(
            [
                (event: "PermissionRequest", command: "/tmp/\(marker)"),
                (event: "Stop", command: "/tmp/\(marker)"),
                (event: "Interrupt", command: "/tmp/\(marker)"),
            ],
            matching: marker,
            into: existing
        )
        let hooks = try XCTUnwrap(merged["hooks"] as? [String: Any])
        XCTAssertTrue(try CodexHookEditing.containsCommand(matching: marker, event: "PermissionRequest", in: merged))
        XCTAssertTrue(try CodexHookEditing.containsCommand(matching: marker, event: "Stop", in: merged))
        XCTAssertTrue(try CodexHookEditing.containsCommand(matching: marker, event: "Interrupt", in: merged))
        XCTAssertEqual(CodexHookEditing.commands(in: try XCTUnwrap(hooks["PermissionRequest"] as? [[String: Any]])), ["/Users/me/policy", "/tmp/\(marker)"])
    }

    func testRemovingMultipleNotificationEventsLeavesUserHooksAndUnknownEvents() throws {
        let root: [String: Any] = [
            "hooks": [
                "PermissionRequest": [["hooks": [["type": "command", "command": "/tmp/\(marker)" ]]]],
                "Stop": [["hooks": [["type": "command", "command": "/Users/me/stop" ]]]],
                "SessionStart": [["hooks": [["type": "command", "command": "/Users/me/start" ]]]],
            ]
        ]
        let cleaned = try CodexHookEditing.removingCommands(
            matching: marker,
            events: CodexHookEditing.notificationEvents,
            from: root
        )
        let hooks = try XCTUnwrap(cleaned["hooks"] as? [String: Any])
        XCTAssertNil(hooks["PermissionRequest"])
        XCTAssertNotNil(hooks["Stop"])
        XCTAssertNotNil(hooks["SessionStart"])
    }

    func testRemovingOnlySumpterCommandKeepsUserHook() throws {
        let rows: [[String: Any]] = [
            ["hooks": [["type": "command", "command": "/Users/me/own-hook"]]],
            ["hooks": [["type": "command", "command": "/tmp/\(marker)"]]]
        ]
        let cleaned = try CodexHookEditing.removingStopCommand(matching: marker, from: root(rows))
        let hooks = try XCTUnwrap(cleaned["hooks"] as? [String: Any])
        let kept = try XCTUnwrap(hooks["Stop"] as? [[String: Any]])
        XCTAssertEqual(CodexHookEditing.commands(in: kept), ["/Users/me/own-hook"])
    }

    func testRemovingSumpterCommandDoesNotTouchUnknownHandlerFields() throws {
        let custom: [[String: Any]] = [[
            "hooks": [[
                "type": "mcp_tool",
                "server": "user-server",
                "tool": "user-tool",
                "command": "/tmp/\(marker)"
            ]]
        ]]
        let cleaned = try CodexHookEditing.removingStopCommand(matching: marker, from: root(custom))
        let hooks = try XCTUnwrap(cleaned["hooks"] as? [String: Any])
        let kept = try XCTUnwrap(hooks["Stop"] as? [[String: Any]])
        XCTAssertEqual(kept.count, 1)
        XCTAssertEqual(CodexHookEditing.commands(in: kept), [])
    }

    func testRemovingLastSumpterCommandDropsStopButKeepsOtherEvents() throws {
        let rows: [[String: Any]] = [["hooks": [["type": "command", "command": "/tmp/\(marker)"]]]]
        let cleaned = try CodexHookEditing.removingStopCommand(matching: marker, from: root(rows))
        let hooks = try XCTUnwrap(cleaned["hooks"] as? [String: Any])
        XCTAssertNil(hooks["Stop"])
        XCTAssertNotNil(hooks["SessionStart"])
    }

    func testMalformedRootIsRejected() {
        XCTAssertThrowsError(try CodexHookEditing.loadRoot(data: Data("[]".utf8))) { error in
            XCTAssertEqual(error as? CodexHookEditingError, .invalidRoot)
        }
        XCTAssertThrowsError(try CodexHookEditing.loadRoot(data: Data("{bad".utf8))) { error in
            XCTAssertEqual(error as? CodexHookEditingError, .invalidJSON)
        }
    }

    func testMalformedStopFieldIsRejected() throws {
        let malformed: [String: Any] = ["hooks": ["Stop": "not-an-array"]]
        XCTAssertThrowsError(try CodexHookEditing.upsertStopCommand("/tmp/\(marker)", matching: marker, into: malformed))
    }

    func testMalformedKnownHookFieldIsRejected() {
        let malformed: [String: Any] = ["hooks": ["SessionStart": "not-an-array"]]
        XCTAssertThrowsError(try CodexHookEditing.upsertStopCommand("/tmp/\(marker)", matching: marker, into: malformed))
        let malformedState: [String: Any] = ["hooks": ["state": "not-an-object"]]
        XCTAssertThrowsError(try CodexHookEditing.upsertStopCommand("/tmp/\(marker)", matching: marker, into: malformedState))
    }

    func testWrongKnownFieldTypesAreRejectedBeforeWriting() {
        let malformedDescription: [String: Any] = ["description": 42]
        XCTAssertThrowsError(try CodexHookEditing.upsertStopCommand("/tmp/\(marker)", matching: marker, into: malformedDescription))

        let malformedMatcher: [String: Any] = [
            "hooks": ["Stop": [["matcher": 42, "hooks": []]]]
        ]
        XCTAssertThrowsError(try CodexHookEditing.upsertStopCommand("/tmp/\(marker)", matching: marker, into: malformedMatcher))

        let malformedCommand: [String: Any] = [
            "hooks": ["Stop": [["hooks": [["type": "command", "command": 42]]]]]
        ]
        XCTAssertThrowsError(try CodexHookEditing.upsertStopCommand("/tmp/\(marker)", matching: marker, into: malformedCommand))

        let malformedTimeout: [String: Any] = [
            "hooks": ["Stop": [["hooks": [["type": "command", "command": "user", "timeout": true]]]]]
        ]
        XCTAssertThrowsError(try CodexHookEditing.upsertStopCommand("/tmp/\(marker)", matching: marker, into: malformedTimeout))
    }
}
