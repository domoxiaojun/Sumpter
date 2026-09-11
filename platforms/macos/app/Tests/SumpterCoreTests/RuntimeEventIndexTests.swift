import Foundation
import XCTest
@testable import SumpterCore

final class RuntimeEventIndexTests: XCTestCase {
    func testCompletedHistoryWinsOverStaleLiveEventInEitherOrder() {
        let completed = event(id: "client", phase: .completed, timestamp: 10)
        let live = event(id: "client", phase: .inFlight, timestamp: 20)

        for events in [[completed, live], [live, completed]] {
            let index = RuntimeEvent.indexedByID(events)
            XCTAssertEqual(index.count, 1)
            XCTAssertEqual(index["client"], completed)
        }
    }

    func testNewerEventWinsAndEqualTimestampsKeepFirstValue() {
        let older = event(id: "client", phase: .inFlight, timestamp: 10)
        var newer = event(id: "client", phase: .inFlight, timestamp: 20)
        newer.durationMS = 200
        var tied = newer
        tied.durationMS = 300

        XCTAssertEqual(RuntimeEvent.indexedByID([older, newer])["client"], newer)
        XCTAssertEqual(RuntimeEvent.indexedByID([newer, older])["client"], newer)
        XCTAssertEqual(RuntimeEvent.indexedByID([newer, tied])["client"], newer)
    }

    func testCompletedProjectionKeepsPreviouslyLoadedDetails() {
        var live = event(id: "client", phase: .inFlight)
        live.toolCalls = ["read_file"]
        var completed = event(id: "client", phase: .completed)
        completed.detailsOmitted = true

        for events in [[completed, live], [live, completed]] {
            let merged = RuntimeEvent.indexedByID(events)["client"]
            XCTAssertEqual(merged?.phase, .completed)
            XCTAssertEqual(merged?.toolCalls, ["read_file"])
        }
        completed.detailsOmitted = false
        XCTAssertNil(RuntimeEvent.indexedByID([live, completed])["client"]?.toolCalls)
    }

    func testSameRequestKeepsDistinctClientAndUpstreamAttempts() {
        var client = event(id: "client", phase: .completed)
        client.requestID = "request"
        var upstream = event(id: "upstream-1", phase: .completed)
        upstream.kind = "upstream"
        upstream.requestID = "request"
        var retry = upstream
        retry.id = "upstream-2"

        let index = RuntimeEvent.indexedByID([client, upstream, retry, client])
        XCTAssertEqual(Set(index.keys), ["client", "upstream-1", "upstream-2"])
    }

    func testLiveOverlayExcludesPersistedIDsAndAppliesKindFilter() {
        let stale = event(id: "persisted", phase: .inFlight)
        let live = event(id: "live", phase: .inFlight)
        var upstream = event(id: "upstream", phase: .inFlight)
        upstream.kind = "upstream"
        let events = [stale, live, upstream, live]

        XCTAssertEqual(
            RuntimeEvent.liveOverlay(events, excluding: ["persisted"], filter: .client),
            [live]
        )
        XCTAssertEqual(
            RuntimeEvent.liveOverlay(events, excluding: ["persisted"], filter: .all),
            [live, upstream]
        )
    }

    func testLiveOverlayDoesNotReviveCompletedDuplicateWithoutHistoryPage() {
        let live = event(id: "client", phase: .inFlight)
        let completed = event(id: "client", phase: .completed)
        XCTAssertTrue(RuntimeEvent.liveOverlay([live, completed], excluding: [], filter: .all).isEmpty)
    }

    private func event(
        id: String,
        phase: RuntimeEventPhase,
        timestamp: TimeInterval = 10
    ) -> RuntimeEvent {
        RuntimeEvent(
            id: id,
            timestamp: Date(timeIntervalSinceReferenceDate: timestamp),
            kind: "client",
            statusCode: phase == .completed ? 200 : 0,
            durationMS: 10,
            outcome: phase == .completed ? .succeeded : nil,
            phase: phase
        )
    }
}
