import Foundation
@testable import SumpterApp
@testable import SumpterCore
import XCTest

final class RuntimeV2WireContractTests: XCTestCase {
    func testStickyKeyRoundTripsAndOmitsWhenAbsent() throws {
        // Rust 事件 wire 的 stickyKey 是会话粘性归属键(affinity 哈希)。
        // 带:解码并回写;不带(早期拒绝/旧事件):保持 nil 且编码省略。
        let data = Data(#"{"id":"sticky-1","timestamp":1.5,"kind":"client","statusCode":200,"durationMS":12,"failover":false,"stickyKey":"affinity-9f2c"}"#.utf8)
        let event = try JSONDecoder().decode(RuntimeEvent.self, from: data)
        XCTAssertEqual(event.stickyKey, "affinity-9f2c")
        let encoded = try JSONEncoder().encode(event)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertEqual(object["stickyKey"] as? String, "affinity-9f2c")

        let legacy = Data(#"{"id":"sticky-0","timestamp":1.5,"kind":"client","statusCode":200,"durationMS":12,"failover":false}"#.utf8)
        let oldEvent = try JSONDecoder().decode(RuntimeEvent.self, from: legacy)
        XCTAssertNil(oldEvent.stickyKey)
        let oldEncoded = try JSONEncoder().encode(oldEvent)
        let oldObject = try XCTUnwrap(JSONSerialization.jsonObject(with: oldEncoded) as? [String: Any])
        XCTAssertNil(oldObject["stickyKey"], "nil 时不得输出键")
    }

    func testSessionStickyTTLHoursDecodesWithDefaultAndClamp() throws {
        // 缺省 = 72h(与 Rust default 对齐);负值/NaN 落盘口径是 0 = 永不过期。
        let absent = try JSONDecoder().decode(AppConfig.self, from: Data(#"{"schemaVersion":7}"#.utf8))
        XCTAssertEqual(absent.sessionStickyTtlHours, 72)

        let withValue = try JSONDecoder().decode(AppConfig.self, from: Data(#"{"schemaVersion":7,"sessionStickyTtlHours":0}"#.utf8))
        XCTAssertEqual(withValue.sessionStickyTtlHours, 0)
        let encoded = try JSONEncoder().encode(withValue)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertEqual(object["sessionStickyTtlHours"] as? Double, 0, "0 也要显式写出,对齐 Rust 契约")
    }

    func testPricingMutationDecodesPutAcknowledgement() throws {
        let data = Data(#"{"revision":7,"currency":"USD","priceCount":3}"#.utf8)
        let value = try JSONDecoder().decode(AdminWire.RuntimePricingMutation.self, from: data)

        XCTAssertEqual(value.revision, 7)
        XCTAssertEqual(value.currency, "USD")
        XCTAssertEqual(value.priceCount, 3)
    }

    func testRuntimeRetentionAllowsOmittedStorageLimit() throws {
        let data = Data(#"{"revision":4}"#.utf8)
        let value = try JSONDecoder().decode(AdminWire.RuntimeRetention.self, from: data)

        XCTAssertEqual(value.revision, 4)
        XCTAssertNil(value.maxAgeDays)
        XCTAssertNil(value.storageLimitBytes)
    }

    func testRuntimeRetentionDecodesTimeAndCapacityDimensions() throws {
        let data = Data(#"{"revision":5,"maxAgeDays":30,"storageLimitBytes":8388608}"#.utf8)
        let value = try JSONDecoder().decode(AdminWire.RuntimeRetention.self, from: data)

        XCTAssertEqual(value.revision, 5)
        XCTAssertEqual(value.maxAgeDays, 30)
        XCTAssertEqual(value.storageLimitBytes, 8_388_608)
    }

    func testRuntimeStorageProbeAllowsLegacyWarningFieldToBeOmitted() throws {
        let data = Data(#"{"apiVersion":3,"backend":"sqlite","schemaVersion":3,"projectionVersion":3,"projectionBackfillCursor":0,"projectionBackfillComplete":true,"projectionIndexesReady":true,"missingIndexes":[],"hourlyRollupComplete":true,"hourlyRollupMaxSeq":0,"hourlyRollupHistoryGeneration":0,"hourlyRollupFailed":false,"hourlyRollupDirtyBuckets":0,"retainedEvents":0,"completedEvents":0,"inFlightEvents":0,"minSeq":null,"maxSeq":null,"earliestTimestamp":null,"latestTimestamp":null,"retainedFromSeq":1,"historyGeneration":0,"resetGeneration":0,"userDeletedEvents":0,"userDeletedRequests":0,"payloadBytes":0,"databaseBytes":0,"liveBytes":0,"allocatedBytes":0,"freelistBytes":0,"walBytes":0,"pendingEvents":0,"pendingBytes":0,"retention":{"revision":1}}"#.utf8)
        let value = try JSONDecoder().decode(AdminWire.RuntimeStorageProbe.self, from: data)

        XCTAssertNil(value.legacyRetentionDetected)
    }

    func testStableHistoryPageDecodesSnapshotContract() throws {
        let data = Data(#"{"apiVersion":3,"events":[],"page":2,"pageSize":50,"totalCount":81,"totalPages":2,"snapshotSeq":900,"historyGeneration":4,"resetGeneration":3,"retainedFromSeq":12,"hasNext":false,"hasPrevious":true,"nextCursor":null,"previousCursor":51,"filters":{"kind":null,"outcome":null,"clientKind":null,"requestPurpose":null,"requestID":null,"endpointID":null,"model":null,"projectID":null,"project":null,"sessionID":null,"failureKind":null,"failurePhase":null,"from":null,"to":null}}"#.utf8)
        let value = try JSONDecoder().decode(AdminWire.RuntimeHistoryPage.self, from: data)

        XCTAssertEqual(value.apiVersion, 3)
        XCTAssertEqual(value.page, 2)
        XCTAssertEqual(value.totalCount, 81)
        XCTAssertEqual(value.snapshotSeq, 900)
        XCTAssertEqual(value.historyGeneration, 4)
        XCTAssertTrue(value.hasPrevious)
        XCTAssertFalse(value.hasNext)
    }

    func testAnalyticsDimensionRowAllowsOmittedLegacyEventIDs() throws {
        let data = Data(#"{"name":"example","attempts":2,"successes":2,"failures":0,"cancelled":0,"pending":0,"successRate":1,"failovers":0,"averageDurationMS":120,"averageTTFBMS":40}"#.utf8)

        let value = try JSONDecoder().decode(
            AdminWire.RuntimeAnalytics.DimensionRow.self,
            from: data
        )

        XCTAssertEqual(value.name, "example")
        XCTAssertNil(value.eventIDs)
    }

    func testAnalyticsV3LatencyUsesAveragesAndThresholdBuckets() throws {
        let data = Data(#"""
        {
          "apiVersion":3,
          "rollupUsed":true,
          "granularity":"hour",
          "from":0,
          "to":3600,
          "snapshotSeq":900,
          "historyGeneration":4,
          "retainedFromSeq":12,
          "thresholds":{"ttfbMS":[5000,15000],"durationMS":[3000,6000]},
          "points":[],
          "totals":{
            "bucketStart":0,"bucketEnd":3600,"clientRequests":2,"clientSuccesses":2,
            "clientFailures":0,"clientCancelled":0,"clientTerminalRequests":2,"clientUnknownResults":0,
            "failovers":0,"failoverTerminalRequests":0,"failoverRecoveredRequests":0,"failoverRecoveryRate":null,
            "upstreamAttempts":0,"upstreamSuccesses":0,"upstreamFailures":0,
            "tokens":{"inputTokens":0,"outputTokens":0,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"reasoningTokens":0,"uncachedInputTokens":0,"processedInputTokens":0,"processedTotalTokens":0,"observedRequests":2,"accountingKnownRequests":2,"accountingUnknownRequests":0,"cacheReadReportedRequests":0,"cacheReadHitRequests":0,"cacheReadTokenEligibleRequests":0,"cacheReadTokenUnknownRequests":0,"cacheReadTokenRate":null,"cacheReadRequestRate":null,"usageFieldPresence":{"inputTokens":2,"outputTokens":2,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"reasoningTokens":0}},
            "ttfbMS":{"observedRequests":2,"sumMS":8000,"averageMS":4000,"thresholdBuckets":[{"thresholdMS":5000,"exceededRequests":1},{"thresholdMS":15000,"exceededRequests":0}]},
            "durationMS":{"observedRequests":2,"sumMS":12000,"averageMS":6000,"thresholdBuckets":[{"thresholdMS":3000,"exceededRequests":2},{"thresholdMS":6000,"exceededRequests":0}]},
            "cost":{"estimatedCostMicros":0,"pricedRequests":0,"unpricedRequests":2,"unknownAccountingRequests":0,"complete":false,"currency":"USD","priceVersion":1}
          },
          "filters":{}
        }
        """#.utf8)
        let value = try JSONDecoder().decode(AdminWire.RuntimeTrendSeries.self, from: data)

        XCTAssertEqual(value.apiVersion, 3)
        XCTAssertEqual(value.thresholds.ttfbMS, [5000, 15000])
        XCTAssertEqual(value.totals.ttfbMS.sumMS, 8000)
        XCTAssertEqual(value.totals.ttfbMS.averageMS, 4000)
        XCTAssertEqual(value.totals.ttfbMS.thresholdBuckets.first?.exceededRequests, 1)
        XCTAssertEqual(value.totals.durationMS.averageMS, 6000)
        XCTAssertEqual(value.totals.durationMS.thresholdBuckets.last?.exceededRequests, 0)
    }

    func testRuntimeFilterKeepsProjectIDAndDisplayAliasIndependent() throws {
        let data = Data(#"{"projectID":"sha256-project","project":"Project Alpha"}"#.utf8)
        let value = try JSONDecoder().decode(AdminWire.RuntimeFilter.self, from: data)

        XCTAssertEqual(value.projectID, "sha256-project")
        XCTAssertEqual(value.project, "Project Alpha")

        let encoded = try JSONEncoder().encode(value)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertEqual(object["projectID"] as? String, "sha256-project")
        XCTAssertEqual(object["project"] as? String, "Project Alpha")
    }
}
