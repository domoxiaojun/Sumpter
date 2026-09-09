import XCTest
@testable import SumpterCore
@testable import SumpterApp

final class RuntimeEventObservationTests: XCTestCase {
    private func data(_ extra: [String: Any] = [:]) throws -> Data {
        let base: [String: Any] = ["id": "r", "kind": "client", "timestamp": 100.0, "statusCode": 200,
                                  "durationMS": 200, "failover": false, "phase": "completed", "outcome": "failed"]
        return try JSONSerialization.data(withJSONObject: base.merging(extra) { _, new in new })
    }

    func testPaginationDecodesSixTopLevelFieldsWithoutCodexMetadata() throws {
        let item = try JSONDecoder().decode(AdminWire.RuntimeEventListItem.self, from: data([
            "seq": 1, "changeSeq": 3, "detailsOmitted": true, "clientVariant": "desktop", "agentRole": "subagent",
            "agentName": "/root/review", "parentThreadId": "parent-thread", "parentTurnId": "parent-turn", "rootTurnId": "root-turn",
            "cacheRead": ["state": "hit", "readTokens": 1280, "finality": "confirmed"],
        ]))
        let event = item.runtimeEvent
        XCTAssertNil(event.codexMetadata)
        XCTAssertEqual(event.clientVariant, "desktop")
        XCTAssertEqual(event.agentRole, "subagent")
        XCTAssertEqual(event.agentName, "/root/review")
        XCTAssertEqual(event.parentThreadID, "parent-thread")
        XCTAssertEqual(event.parentTurnID, "parent-turn")
        XCTAssertEqual(event.rootTurnID, "root-turn")
        XCTAssertEqual(event.statusCode, 200)
        XCTAssertTrue(event.isFailed)
        XCTAssertTrue(event.cacheReadLabel.contains("已命中"))
    }

    func testOmittedProjectionPreservesTraceButFullEventCanClearIt() throws {
        let full = try JSONDecoder().decode(RuntimeEvent.self, from: data(["streamTrace": ["usage": ["inputTokens": 20]]]))
        let page = try JSONDecoder().decode(AdminWire.RuntimeEventListItem.self, from: data(["seq": 1, "changeSeq": 4, "detailsOmitted": true]))
        XCTAssertEqual(page.mergedRuntimeEvent(with: full).streamTrace?.usage?.inputTokens, 20)
        let replacement = try JSONDecoder().decode(AdminWire.RuntimeEventListItem.self, from: data(["seq": 1, "changeSeq": 5, "detailsOmitted": false]))
        XCTAssertNil(replacement.mergedRuntimeEvent(with: full).streamTrace)
    }

    func testConfirmedHitDoesNotInventAnUnknownInputDenominator() throws {
        var event = try JSONDecoder().decode(RuntimeEvent.self, from: data([
            "targetFormat": "anthropic", "cacheRead": ["state": "hit", "readTokens": 20, "finality": "confirmed"],
            "usageSummary": ["inputTokens": 80]
        ]))
        XCTAssertNil(event.cacheReadTokenRatio)
        event.usageSummary?.cacheCreationInputTokens = 0
        XCTAssertEqual(event.cacheReadTokenRatio, 0.2)
        XCTAssertEqual(try JSONDecoder().decode(RuntimeEvent.self, from: data()).cacheReadLabel, "缓存未知")
    }
}
