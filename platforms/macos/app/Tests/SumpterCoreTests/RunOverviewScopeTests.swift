import Foundation
import SumpterCore
import XCTest
@testable import SumpterApp

@MainActor
final class RunOverviewScopeTests: XCTestCase {
    func testRunHistoryUsesLocalDayAndOnlyKeepsSameDaySnapshotBounds() throws {
        let model = AppModel(startServices: false)
        model.runtimeAnalyticsRange = "30d"
        model.runHistoryKindFilter = "upstream"
        model.runHistoryOutcomeFilter = "failed"
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "Asia/Singapore"))
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-11-04T12:00:00+08:00"))
        let initial = model.runHistoryFilter(now: now, calendar: calendar)
        XCTAssertEqual(RunOverviewScope.range, "today")
        XCTAssertEqual(initial.from, calendar.startOfDay(for: now).timeIntervalSinceReferenceDate)
        XCTAssertEqual(initial.kind, "upstream")
        XCTAssertEqual(initial.outcome, "failed")
        let later = now.addingTimeInterval(3600)
        XCTAssertEqual(model.runHistoryFilter(snapshot: initial, now: later, calendar: calendar).from, initial.from)
        XCTAssertEqual(model.runHistoryFilter(now: later, calendar: calendar).from, initial.from)
        let bounded = AdminWire.RuntimeFilter(from: initial.from, to: now.timeIntervalSinceReferenceDate)
        XCTAssertEqual(model.runHistoryFilter(snapshot: bounded, now: later, calendar: calendar).to, bounded.to)
        let tomorrow = now.addingTimeInterval(86_400)
        XCTAssertFalse(RunOverviewScope.isCurrent(bounded, now: tomorrow, calendar: calendar))
        let refreshed = model.runHistoryFilter(snapshot: bounded, now: tomorrow, calendar: calendar)
        XCTAssertEqual(refreshed.from, calendar.startOfDay(for: tomorrow).timeIntervalSinceReferenceDate)
        XCTAssertNil(refreshed.to)
    }

    func testCachedAndLiveEventsCannotEscapeTodayWindow() throws {
        let now = Date(timeIntervalSinceReferenceDate: 800_000_000)
        let filter = RunOverviewScope.filter(now: now)
        let from = try XCTUnwrap(filter.from)
        for phase in ["completed", "inFlight"] {
            func event(_ timestamp: Double) throws -> RuntimeEvent {
                let value: [String: Any] = ["id": "test", "timestamp": timestamp, "kind": "client",
                                           "statusCode": 200, "durationMS": 1, "failover": false, "phase": phase]
                return try JSONDecoder().decode(RuntimeEvent.self, from: JSONSerialization.data(withJSONObject: value))
            }
            XCTAssertFalse(RunOverviewScope.contains(try event(from - 0.001), filter: filter))
            XCTAssertTrue(RunOverviewScope.contains(try event(from), filter: filter))
            XCTAssertTrue(RunOverviewScope.contains(try event(now.timeIntervalSinceReferenceDate), filter: filter))
            XCTAssertFalse(RunOverviewScope.contains(try event(from + 2), filter: AdminWire.RuntimeFilter(from: from, to: from + 1)))
        }
    }

    func testMidnightInvalidatesOldPageAndPendingReads() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "Asia/Singapore"))
        let before = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-11-04T23:59:59+08:00"))
        let model = AppModel(startServices: false)
        model.advanceRunOverviewDay(now: before, calendar: calendar)
        let oldFrom = model.runOverviewDayStart
        let value: [String: Any] = [
            "apiVersion": 3, "events": [], "page": 2, "pageSize": 10,
            "totalCount": 11, "totalPages": 2, "snapshotSeq": 42, "snapshotChangeSeq": 42,
            "historyGeneration": 0, "resetGeneration": 0, "retainedFromSeq": 1,
            "hasNext": false, "hasPrevious": true, "filters": ["from": oldFrom]
        ]
        model.runHistoryPage = try JSONDecoder().decode(AdminWire.RuntimeHistoryPage.self, from: JSONSerialization.data(withJSONObject: value))
        model.runHistoryLoading = true
        model.runOverviewFailed = true
        let pendingHistory = model.runHistoryRequestGeneration
        let pendingOverview = model.runOverviewRequestGeneration
        XCTAssertFalse(model.advanceRunOverviewDay(now: before, calendar: calendar))
        XCTAssertEqual(model.runHistoryPage?.page, 2)
        XCTAssertTrue(model.advanceRunOverviewDay(now: before.addingTimeInterval(1), calendar: calendar))
        XCTAssertEqual(model.runOverviewDayStart, oldFrom + 86_400)
        XCTAssertNil(model.runHistoryPage)
        XCTAssertFalse(model.runHistoryLoading)
        XCTAssertFalse(model.runOverviewFailed)
        XCTAssertGreaterThan(model.runHistoryRequestGeneration, pendingHistory)
        XCTAssertGreaterThan(model.runOverviewRequestGeneration, pendingOverview)
    }

    func testLocalDayFollowsDaylightSavingTransitions() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "America/New_York"))
        for (before, after, hours) in [
            ("2026-03-08T12:00:00-04:00", "2026-03-09T12:00:00-04:00", 23.0),
            ("2026-11-01T12:00:00-05:00", "2026-11-02T12:00:00-05:00", 25.0)
        ] {
            let first = try XCTUnwrap(ISO8601DateFormatter().date(from: before))
            let second = try XCTUnwrap(ISO8601DateFormatter().date(from: after))
            XCTAssertEqual(RunOverviewScope.dayStart(now: second, calendar: calendar) - RunOverviewScope.dayStart(now: first, calendar: calendar), hours * 3600)
        }
    }

    func testHomeTimestampOmitsDateWhileHistoryAndDetailsKeepIt() {
        let old = Date(timeIntervalSinceReferenceDate: 0)
        let time = RuntimeEventDisplay.time(old, includeDate: false)
        XCTAssertNotNil(time.range(of: #"^\d{2}:\d{2}:\d{2}$"#, options: .regularExpression))
        XCTAssertGreaterThan(RuntimeEventDisplay.time(old).count, time.count)
        XCTAssertGreaterThan(RuntimeEventDisplay.dateTime(old).count, time.count)
    }
}
