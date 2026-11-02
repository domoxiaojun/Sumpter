import Foundation
@testable import SumpterApp
@testable import SumpterCore
import XCTest

final class RuntimeLiveSyncTests: XCTestCase {
    func testAll250LiveRequestsSurviveRefreshAndIncrementalTrimming() throws {
        let live = try (0..<250).map { try event(String($0), phase: "inFlight") }
        let refreshed = try summary(live).reconcilingInFlightEvents(in: [])
        let updated = RuntimeEvent.trimmed([try event("new", phase: "completed")] + refreshed, perKindLimit: 200)
        XCTAssertEqual(updated.filter(\.isInFlight).count, 250)
        XCTAssertTrue(updated.contains { $0.id == "249" })
    }

    func testCompletedOverlaySurvivesBeforeCommitAndKeepsOlderPagesStable() throws {
        let pageJSON = #"{"apiVersion":3,"events":[],"page":1,"pageSize":10,"totalCount":0,"totalPages":0,"snapshotSeq":0,"snapshotChangeSeq":10,"historyGeneration":0,"resetGeneration":0,"retainedFromSeq":0,"hasNext":false,"hasPrevious":false,"filters":{"kind":"client"}}"#
        let decoder = JSONDecoder()
        let page = try decoder.decode(AdminWire.RuntimeHistoryPage.self, from: Data(pageJSON.utf8))
        let done = try decoder.decode(AdminWire.RuntimeEventListItem.self, from: Data(#"{"seq":1,"changeSeq":11,"id":"late","timestamp":100,"kind":"client","phase":"completed","statusCode":101,"durationMS":1000,"failover":false}"#.utf8))
        XCTAssertEqual(page.visibleEvents(recent: [done]).map(\.id), ["late"])
        let oldPage = try decoder.decode(AdminWire.RuntimeHistoryPage.self, from: Data(pageJSON.replacingOccurrences(of: "\"page\":1", with: "\"page\":2").utf8))
        XCTAssertTrue(oldPage.visibleEvents(recent: [done]).isEmpty)
    }

    private func event(_ id: String, phase: String) throws -> RuntimeEvent {
        try JSONDecoder().decode(RuntimeEvent.self, from: Data("""
        {"id":"\(id)","timestamp":1,"kind":"client","phase":"\(phase)","statusCode":200,"durationMS":0,"failover":false}
        """.utf8))
    }

    private func summary(_ live: [RuntimeEvent]?) throws -> AdminWire.RuntimeSummary {
        let data = Data(#"{"apiVersion":1,"storage":{"backend":"sqlite","state":"ready","pendingEvents":0,"eventCount":0,"dbBytes":0,"walBytes":0},"resetGeneration":0,"counters":{"clientRequests":0,"clientSuccesses":0,"clientFailures":0,"upstreamAttempts":0,"upstreamSuccesses":0,"upstreamFailures":0,"failovers":0}}"#.utf8)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        if let live { object["inFlightEvents"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(live)) }
        return try JSONDecoder().decode(AdminWire.RuntimeSummary.self, from: JSONSerialization.data(withJSONObject: object))
    }

    func testLongRetrySurvivesNewestPageRefreshAndClearsWhenFinished() throws {
        let live = try event("long-retry", phase: "inFlight")
        let page = try (0..<10).map { try event(String($0), phase: "completed") }
        let restored = try summary([live]).reconcilingInFlightEvents(in: page)
        XCTAssertEqual(restored.map(\.id), [live.id] + page.map(\.id))
        XCTAssertEqual(try summary([]).reconcilingInFlightEvents(in: restored).map(\.id), page.map(\.id))
    }

    func testCompletedEventWinsAndOldDaemonRemainsCompatible() throws {
        let live = try event("request", phase: "inFlight")
        let completed = try event("request", phase: "completed")
        let result = try summary([live]).reconcilingInFlightEvents(in: [completed])
        XCTAssertEqual(result.count, 1)
        XCTAssertFalse(try XCTUnwrap(result.first).isInFlight)
        XCTAssertEqual(try summary(nil).reconcilingInFlightEvents(in: [live]).map(\.id), [live.id])
    }
}
