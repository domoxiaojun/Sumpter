import XCTest
@testable import SumpterCore

/// 覆盖 Claude Code hooks 数组的合并/删除逻辑,重点保证不覆盖用户已有 hooks。
final class ClaudeHookEditingTests: XCTestCase {
    private let needle = "kekulv-notify.sh"

    private func userRow(_ command: String) -> [String: Any] {
        ["hooks": [["type": "command", "command": command]]]
    }

    func testUpsertPreservesExistingUserHooks() {
        let existing: [[String: Any]] = [userRow("/Users/me/my-own-hook.sh")]
        let merged = ClaudeHookEditing.upsertCommand(
            "/bin/zsh /path/kekulv-notify.sh stop",
            matching: needle,
            into: existing
        )
        let commands = ClaudeHookEditing.commands(in: merged)
        XCTAssertTrue(commands.contains("/Users/me/my-own-hook.sh"), "用户已有 hook 必须保留")
        XCTAssertTrue(commands.contains { $0.contains(needle) }, "Sumpter命令应已追加")
        XCTAssertEqual(commands.count, 2)
    }

    func testUpsertReplacesOldSumpterCommandWithoutDuplicating() {
        let existing: [[String: Any]] = [
            userRow("/Users/me/my-own-hook.sh"),
            userRow("/old/path/kekulv-notify.sh stop")
        ]
        let merged = ClaudeHookEditing.upsertCommand(
            "/new/path/kekulv-notify.sh stop",
            matching: needle,
            into: existing
        )
        let kekulv = ClaudeHookEditing.commands(in: merged).filter { $0.contains(needle) }
        XCTAssertEqual(kekulv, ["/new/path/kekulv-notify.sh stop"], "旧Sumpter命令应被替换,不重复")
        XCTAssertTrue(ClaudeHookEditing.commands(in: merged).contains("/Users/me/my-own-hook.sh"))
    }

    func testRemovingKeepsUserHooksAndDropsSumpter() {
        let existing: [[String: Any]] = [
            userRow("/Users/me/my-own-hook.sh"),
            userRow("/path/kekulv-notify.sh stop")
        ]
        let cleaned = ClaudeHookEditing.removingCommands(matching: needle, from: existing)
        let commands = ClaudeHookEditing.commands(in: cleaned)
        XCTAssertEqual(commands, ["/Users/me/my-own-hook.sh"])
    }

    func testRemovingDropsEmptiedMatcherRows() {
        let existing: [[String: Any]] = [userRow("/path/kekulv-notify.sh stop")]
        let cleaned = ClaudeHookEditing.removingCommands(matching: needle, from: existing)
        XCTAssertTrue(cleaned.isEmpty, "只剩Sumpter命令的行应整行移除,以便调用方删掉该事件键")
    }

    func testContainsCommand() {
        let existing: [[String: Any]] = [userRow("/path/kekulv-notify.sh stop")]
        XCTAssertTrue(ClaudeHookEditing.containsCommand(matching: needle, in: existing))
        XCTAssertFalse(ClaudeHookEditing.containsCommand(matching: needle, in: [userRow("/x/other.sh")]))
        XCTAssertFalse(ClaudeHookEditing.containsCommand(matching: needle, in: nil))
    }

    func testUpsertIntoNilOrEmptyStartsFresh() {
        let merged = ClaudeHookEditing.upsertCommand("/path/kekulv-notify.sh notification", matching: needle, into: nil)
        XCTAssertEqual(ClaudeHookEditing.commands(in: merged), ["/path/kekulv-notify.sh notification"])
    }
}
