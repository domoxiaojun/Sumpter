import Foundation
@testable import SumpterApp
@testable import SumpterCore
import XCTest

final class ResponsesConnectionsTests: XCTestCase {
    private func connections(wait: Int? = nil) throws -> ResponsesWebSocketConnections {
        let data = Data("""
        {"total":\(wait == nil ? 0 : 8),"awaitingFirstMessage":\(wait == nil ? 0 : 8),"guardianAwaitingFirstMessage":\(wait == nil ? 0 : 8),"connectingUpstream":0,"relaying":0,"oldestFirstMessageWaitMS":\(wait.map(String.init) ?? "null")}
        """.utf8)
        return try JSONDecoder().decode(ResponsesWebSocketConnections.self, from: data)
    }

    func testMissingFieldIsCompatibleWithOlderRuntimeSummary() throws {
        let data = Data(#"{"apiVersion":1,"storage":{"backend":"sqlite","state":"ready","pendingEvents":0,"eventCount":0,"dbBytes":0,"walBytes":0},"resetGeneration":0,"counters":{"clientRequests":0,"clientSuccesses":0,"clientFailures":0,"upstreamAttempts":0,"upstreamSuccesses":0,"upstreamFailures":0,"failovers":0}}"#.utf8)
        let old = try JSONDecoder().decode(AdminWire.RuntimeSummary.self, from: data)
        XCTAssertNil(old.responsesWebSocketConnections)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        object["responsesWebSocketConnections"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(connections(wait: 1200)))
        let current = try JSONDecoder().decode(AdminWire.RuntimeSummary.self, from: JSONSerialization.data(withJSONObject: object))
        XCTAssertEqual(current.responsesWebSocketConnections?.total, 8)
        XCTAssertEqual(current.responsesWebSocketConnections?.oldestFirstMessageWaitMS, 1200)
    }

    func testUnavailableAndFailureAreDistinctFromZero() throws {
        var observation = ResponsesConnectionsObservation()
        XCTAssertEqual(observation.emptyMessage, "正在读取连接数据…")
        observation.receive(nil)
        XCTAssertEqual(observation.emptyMessage, "当前版本未提供")
        observation.fail("offline")
        XCTAssertEqual(observation.emptyMessage, "连接数据暂不可用")
        observation.receive(try connections())
        XCTAssertNil(observation.emptyMessage)
        XCTAssertNil(observation.errorMessage)
        XCTAssertEqual(observation.value?.total, 0)
        XCTAssertNil(observation.value?.oldestFirstMessageWaitMS)
    }

    func testFailureAndPauseRetainMeasurementAndResumeUsesCurrentWait() throws {
        var observation = ResponsesConnectionsObservation()
        observation.receive(try connections(wait: 1200))
        let frozen = observation
        observation.fail("offline")
        XCTAssertEqual(observation.value, frozen.value)
        XCTAssertEqual(observation.errorMessage, "更新失败，保留上次数据")
        observation.receive(try connections(wait: 2500))
        XCTAssertEqual(observation.displayed(autoRefresh: false, frozen: frozen), frozen)
        XCTAssertEqual(observation.displayed(autoRefresh: true, frozen: frozen).value?.oldestFirstMessageWaitMS, 2500)
        XCTAssertNil(observation.errorMessage)
    }
}
