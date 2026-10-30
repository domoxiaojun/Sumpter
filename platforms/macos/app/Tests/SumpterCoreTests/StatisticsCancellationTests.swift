import XCTest
@testable import SumpterApp

@MainActor
final class StatisticsCancellationTests: XCTestCase {
    func testLeavingStatisticsCancelsOutstandingReads() async {
        let model = AppModel(startServices: false)
        model.setStatisticsVisible(true)
        var cancelled = 0
        let tasks = (0..<5).map { _ in
            Task {
                do {
                    try await Task.sleep(nanoseconds: 60_000_000_000)
                    XCTFail("Superseded statistics reads must not finish normally")
                } catch {
                    cancelled += 1
                }
            }
        }
        model.runtimeSnapshotTask = tasks[0]
        model.runtimeFacetsTask = tasks[1]
        model.runtimeDimensionTask = tasks[2]
        model.runtimeErrorTask = tasks[3]
        model.runtimeRequestChainTask = tasks[4]
        model.setStatisticsVisible(false)
        for task in tasks { await task.value }
        XCTAssertEqual(cancelled, 5)
        XCTAssertFalse(model.statisticsVisible)
        XCTAssertNil(model.runtimeSnapshotTask)
        XCTAssertNil(model.runtimeFacetsTask)
        XCTAssertNil(model.runtimeDimensionTask)
        XCTAssertNil(model.runtimeErrorTask)
        XCTAssertNil(model.runtimeRequestChainTask)
    }
}
