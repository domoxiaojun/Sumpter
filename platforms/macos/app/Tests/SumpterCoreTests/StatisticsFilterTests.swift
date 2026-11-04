import Foundation
import XCTest
@testable import SumpterApp

@MainActor
final class StatisticsFilterTests: XCTestCase {
    func testEveryRangeReachesTheCommonFilter() throws {
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-11-04T04:00:00Z"))
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "Asia/Singapore"))
        let model = AppModel(startServices: false)
        for (range, seconds) in [("7d", 7.0 * 86_400), ("30d", 30.0 * 86_400)] {
            model.runtimeAnalyticsRange = range
            XCTAssertEqual(model.runtimeV2Filter(now: now, calendar: calendar).from, now.timeIntervalSinceReferenceDate - seconds)
        }
        model.runtimeAnalyticsRange = "today"
        XCTAssertEqual(model.runtimeV2Filter(now: now, calendar: calendar).from, calendar.startOfDay(for: now).timeIntervalSinceReferenceDate)
        let tomorrow = now.addingTimeInterval(86_400)
        XCTAssertEqual(model.runtimeV2Filter(now: tomorrow, calendar: calendar).from, calendar.startOfDay(for: tomorrow).timeIntervalSinceReferenceDate)
        model.runtimeAnalyticsRange = "all"
        XCTAssertNil(model.runtimeV2Filter(now: now, calendar: calendar).from)
    }

    func testLocalDrillDownKeepsEveryOtherFilterAndSnapshotBounds() throws {
        let model = AppModel(startServices: false)
        model.runtimeAnalyticsRange = "7d"
        model.runtimeAnalyticsClientKind = "codex"
        model.runtimeAnalyticsRequestKind = "review"
        model.runtimeAnalyticsClientVariant = "desktop"
        model.runtimeAnalyticsAgentRole = "subagent"
        model.runtimeAnalyticsAgentName = "reviewer"
        model.runtimeAnalyticsParentThreadID = "parent"
        model.runtimeAnalyticsParentTurnID = "turn"
        model.runtimeAnalyticsRootTurnID = "root"
        model.runtimeAnalyticsEndpointID = "endpoint"
        model.runtimeAnalyticsModel = "model"
        model.runtimeAnalyticsOutcome = "failed"
        model.runtimeAnalyticsFailureKind = "upstream_http_status"
        let snapshot = AdminWire.RuntimeFilter(from: 100, to: 200)
        var expected = model.runtimeV2Filter(snapshot: snapshot)
        model.runtimeLocalProjectID = "project-id"
        model.runtimeLocalProjectName = "Renamed project"
        model.runtimeLocalSessionID = "session-id"
        expected.projectID = "project-id"
        expected.project = nil
        expected.sessionID = "session-id"
        let actual = model.runtimeV2Filter(includeLocalProject: true, snapshot: snapshot)
        XCTAssertEqual(actual, expected)
        let wire = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(actual)) as? [String: Any])
        for field in ["requestKind", "clientVariant", "agentRole", "agentName", "parentThreadID", "parentTurnID", "rootTurnID"] {
            XCTAssertNotNil(wire[field], field)
        }
        model.runtimeLocalProjectID = "unidentified_project"
        model.runtimeLocalProjectName = "unidentified_project"
        let unidentified = model.runtimeV2Filter(includeLocalProject: true, snapshot: snapshot)
        XCTAssertNil(unidentified.projectID)
        XCTAssertEqual(unidentified.project, "unidentified_project")
        XCTAssertEqual(unidentified.agentName, "reviewer")
    }
}
