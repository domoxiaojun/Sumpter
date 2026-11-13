import Foundation
import XCTest
@testable import SumpterCore
@testable import SumpterApp

final class RuntimeResourceTests: XCTestCase {
    func testDateRangeMatchesSharedFixtures() throws {
        var root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        for _ in 0..<5 { root.deleteLastPathComponent() }
        let rows = try JSONDecoder().decode([Fixture].self, from: Data(contentsOf: root.appendingPathComponent("tests/fixtures/statistics-date-ranges.json")))
        let parser = ISO8601DateFormatter()
        for row in rows {
            var calendar = Calendar(identifier: .gregorian)
            calendar.timeZone = try XCTUnwrap(TimeZone(identifier: row.zone))
            let now = try XCTUnwrap(parser.date(from: row.now))
            let range = StatisticsDateRange(startDay: row.startDay, endDay: row.endDay)
            let bounds = try XCTUnwrap(range.bounds(now: now, calendar: calendar))
            XCTAssertEqual(bounds.from, row.from, row.zone)
            XCTAssertEqual(bounds.to, row.to, row.zone)
            XCTAssertLessThan(bounds.to, row.nextMidnight)
        }
    }

    func testInvalidDatesAndCacheBudget() {
        let invalid = StatisticsDateRange(startDay: "2026-02-30", endDay: "2026-03-01")
        XCTAssertNil(invalid.bounds())
        var cache = BoundedLRUCache<String, String>(countLimit: 2, costLimit: 10)
        cache.insert("a", for: "a", cost: 4)
        cache.insert("b", for: "b", cost: 4)
        XCTAssertEqual(cache.value(for: "a"), "a")
        cache.insert("c", for: "c", cost: 4)
        XCTAssertNil(cache.peek("b"))
        cache.insert("oversize", for: "large", cost: 11)
        XCTAssertEqual(cache.count, 2)
        for index in 0..<1000 { cache.insert("value", for: String(index), cost: 3) }
        XCTAssertLessThanOrEqual(cache.cost, 10)
        cache.removeAll()
        XCTAssertEqual(cache.cost, 0)
    }

    @MainActor
    func testHiddenLifecycleCancelsAndReleasesOnlyRuntimeState() async throws {
        let model = AppModel(startServices: false)
        model.runtimeViewState.windowVisible = true
        model.runtimeViewState.section = .run
        let originalConfig = model.config
        let detailTask = Task<Void, Never> { try? await Task.sleep(for: .seconds(60)) }
        let overviewTask = Task<Void, Never> { try? await Task.sleep(for: .seconds(60)) }
        let statusTask = Task<Void, Never> { try? await Task.sleep(for: .seconds(60)) }
        let estimateTask = Task<Void, Never> { try? await Task.sleep(for: .seconds(60)) }
        model.runtimeEventDetailTask = detailTask
        model.runOverviewTask = overviewTask
        model.statusRefreshTask = statusTask
        model.runtimeExportEstimateTask = estimateTask
        model.setRuntimeWindowVisible(false)
        XCTAssertTrue(detailTask.isCancelled)
        XCTAssertTrue(overviewTask.isCancelled)
        XCTAssertTrue(statusTask.isCancelled)
        XCTAssertTrue(estimateTask.isCancelled)
        XCTAssertFalse(model.runtimeDataVisible)
        XCTAssertTrue(model.runtimeViewState.retainsPageContent)
        let release = model.runtimeCacheReleaseTask
        model.setRuntimeWindowVisible(true)
        XCTAssertTrue(release?.isCancelled == true)
        model.setRuntimeWindowVisible(false)
        model.releaseRuntimePageCaches()
        XCTAssertFalse(model.runtimeViewState.retainsPageContent)
        XCTAssertNil(model.runtimeSummary)
        XCTAssertEqual(model.config, originalConfig)
        model.pollingTask?.cancel()
        model.runtimeCacheReleaseTask?.cancel()
        await detailTask.value
        await overviewTask.value
        await statusTask.value
        await estimateTask.value
    }

    private struct Fixture: Decodable {
        let zone: String
        let startDay: String
        let endDay: String
        let now: String
        let from: Double
        let to: Double
        let nextMidnight: Double
    }
}
