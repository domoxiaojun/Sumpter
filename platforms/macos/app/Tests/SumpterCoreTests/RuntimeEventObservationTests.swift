import XCTest
@testable import SumpterCore
@testable import SumpterApp

final class RuntimeEventObservationTests: XCTestCase {
    func testEventListShowsLogicalModelInsteadOfClientOrUpstreamModel() throws {
        var event = try JSONDecoder().decode(RuntimeEvent.self, from: data([
            "clientModel": "claude-opus-5", "effectiveModel": "gpt-5.6-terra", "upstreamModel": "provider-alias"
        ]))
        XCTAssertEqual(RuntimeEventDisplay.logicalModel(event), "gpt-5.6-terra")
        event.effectiveModel = nil
        XCTAssertEqual(RuntimeEventDisplay.logicalModel(event), "—")
    }

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
        XCTAssertEqual(event.cacheReadLabel, "缓存读取 1,280")
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
        XCTAssertEqual(try JSONDecoder().decode(RuntimeEvent.self, from: data()).cacheReadLabel, "缓存读取 —")
    }

    func testUsageDistinguishesPendingMissingZeroAndPartialReports() throws {
        let pending = try JSONDecoder().decode(RuntimeEvent.self, from: data([
            "phase": "inFlight", "statusCode": 0, "usageSummary": [:]
        ]))
        XCTAssertFalse(pending.hasObservedUsage)
        XCTAssertEqual(pending.usageSummaryLabel, "等待用量")
        let missing = try JSONDecoder().decode(RuntimeEvent.self, from: data())
        XCTAssertFalse(missing.hasObservedUsage)
        XCTAssertEqual(missing.usageSummaryLabel, "未报告用量")

        let zero = try JSONDecoder().decode(RuntimeEvent.self, from: data([
            "phase": "inFlight", "usageSummary": ["inputTokens": 0],
            "cacheRead": ["state": "miss", "readTokens": 0, "finality": "confirmed"]
        ]))
        XCTAssertTrue(zero.hasObservedUsage)
        XCTAssertEqual(zero.usageSummaryLabel, "输入 0 · 输出 —")
        XCTAssertEqual(zero.cacheReadLabel, "缓存读取 0")

        let partial = try JSONDecoder().decode(RuntimeEvent.self, from: data([
            "phase": "inFlight", "streamTrace": ["usage": ["inputTokens": 33, "outputTokens": 3]],
            "cacheRead": ["state": "hit", "readTokens": 222950, "finality": "provisional"]
        ]))
        XCTAssertTrue(partial.hasObservedUsage)
        XCTAssertEqual(partial.usageSummaryLabel, "输入 33 · 输出 3")
        XCTAssertEqual(partial.cacheReadLabel, "缓存读取 222,950")
        XCTAssertEqual(partial.cacheRead?.finality, "provisional")
    }

    func testInlineCacheHitRateUsesTokenShareAndPreservesUnknownEvidence() throws {
        var event = try JSONDecoder().decode(RuntimeEvent.self, from: data([
            "targetFormat": "openai-responses", "cacheRead": ["state": "hit", "readTokens": 203776, "finality": "confirmed"],
            "usageSummary": ["inputTokens": 204082]
        ]))
        XCTAssertEqual(event.cacheReadHitRateLabel, "命中率 99.9%")
        event.cacheRead?.state = "miss"
        event.cacheRead?.readTokens = 0
        XCTAssertEqual(event.cacheReadHitRateLabel, "命中率 0%")
        event.usageSummary?.inputTokens = 0
        XCTAssertEqual(event.cacheReadHitRateLabel, "命中率 —")
        event.usageSummary?.inputTokens = 60
        event.cacheRead?.readTokens = 20
        event.cacheRead?.state = "hit"
        event.cacheRead?.finality = "provisional"
        XCTAssertEqual(event.cacheReadHitRateLabel, "命中率 —")
        event.cacheRead?.finality = "confirmed"
        event.targetFormat = .anthropic
        XCTAssertEqual(event.cacheReadHitRateLabel, "命中率 —")
        event.usageSummary?.cacheCreationInputTokens = 20
        XCTAssertEqual(event.cacheReadHitRateLabel, "命中率 20%")
        XCTAssertEqual(try JSONDecoder().decode(RuntimeEvent.self, from: data()).cacheReadHitRateLabel, "命中率 —")
    }

    func testInFlightStateUsesResponseEvidence() throws {
        var event = try JSONDecoder().decode(RuntimeEvent.self, from: data(["phase": "inFlight", "statusCode": 0]))
        XCTAssertEqual(RuntimeEventDisplay.friendlyMessage(event), "等待响应")
        event.statusCode = 200
        XCTAssertEqual(RuntimeEventDisplay.friendlyMessage(event), "接收响应中")
        event.streamTrace = StreamTrace(chunkCount: 4)
        XCTAssertEqual(RuntimeEventDisplay.friendlyMessage(event), "流式输出中")
        event.statusCode = 0
        event.upstreamStatusCode = 200
        XCTAssertEqual(RuntimeEventDisplay.friendlyMessage(event), "等待响应")
    }
}
