import XCTest
@testable import SumpterApp
@testable import SumpterCore

final class RequestChainRefreshTests: XCTestCase {
    private func item(_ id: String, seq: Int, change: Int, phase: String = "inFlight") throws -> AdminWire.RuntimeEventListItem {
        let data = try JSONSerialization.data(withJSONObject: ["id": id, "seq": seq, "changeSeq": change, "requestID": "r",
            "kind": id == "r" ? "client" : "upstream", "timestamp": 1, "statusCode": 0, "durationMS": 0, "failover": false, "phase": phase])
        return try JSONDecoder().decode(AdminWire.RuntimeEventListItem.self, from: data)
    }

    func testChainKeepsTerminalAgainstOlderSnapshotAndAppendsAttempts() throws {
        let live = AdminWire.RuntimeRequestChain(apiVersion: 2, requestID: "r", events: [try item("r", seq: 1, change: 10, phase: "completed")], truncated: false)
        let older = AdminWire.RuntimeRequestChain(apiVersion: 2, requestID: "r", events: [try item("r", seq: 1, change: 1), try item("a", seq: 2, change: 2)], truncated: false)
        let merged = older.merging(live)
        XCTAssertFalse(merged.isInFlight)
        XCTAssertEqual(merged.events.count, 2)
        XCTAssertEqual(merged.events[0].changeSeq, 10)
        let other = AdminWire.RuntimeRequestChain(apiVersion: 2, requestID: "other", events: [], truncated: false)
        XCTAssertEqual(merged.merging(other), merged)
    }

    func testLongChainRemainsBoundedAndTruncationIsExplicit() throws {
        let events = try (1...600).map { try item($0 == 1 ? "r" : "a\($0)", seq: $0, change: $0) }
        let a = AdminWire.RuntimeRequestChain(apiVersion: 2, requestID: "r", events: Array(events.prefix(512)), truncated: true)
        let b = AdminWire.RuntimeRequestChain(apiVersion: 2, requestID: "r", events: Array(events.suffix(88)), truncated: false)
        XCTAssertEqual(a.merging(b).events.count, 512)
        XCTAssertTrue(a.merging(b).truncated)
        XCTAssertTrue(a.merging(b).isInFlight)
    }

    func testNewWaitSettingAndFailureKindsRoundTripWithoutChangingLegacyDefaults() throws {
        XCTAssertEqual(RequestPolicy().responseWaitTimeoutSeconds, 0)
        var policy = RequestPolicy()
        policy.responseWaitTimeoutSeconds = 200
        XCTAssertEqual(try JSONDecoder().decode(RequestPolicy.self, from: JSONEncoder().encode(policy)), policy)
        for kind in [RuntimeFailureKind.clientDisconnected, .responseWaitTimeout, .clientCancelled] {
            XCTAssertEqual(try JSONDecoder().decode(RuntimeFailureKind.self, from: JSONEncoder().encode(kind)), kind)
        }
        XCTAssertEqual(RuntimeEventPresentation.friendlyMessage(kind: "client", statusCode: 499, failover: false,
            message: nil, outcome: .cancelled, failureKind: .clientDisconnected), "连接中断（原因未确定）")
    }

    @MainActor
    func testStoppingSelectionCancelsPendingReadAndInvalidatesResponse() async {
        let model = AppModel(startServices: false)
        let task = Task<Void, Never> { _ = try? await Task.sleep(nanoseconds: 60_000_000_000) }
        model.runtimeRequestChainTask = task
        model.runtimeRequestChainLoading = true
        let generation = model.runtimeRequestChainRequestGeneration
        model.stopRuntimeRequestChainRefresh()
        await task.value
        XCTAssertTrue(task.isCancelled)
        XCTAssertNil(model.runtimeRequestChainTask)
        XCTAssertFalse(model.runtimeRequestChainLoading)
        XCTAssertGreaterThan(model.runtimeRequestChainRequestGeneration, generation)
    }
}
