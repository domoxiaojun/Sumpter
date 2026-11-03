import XCTest
@testable import SumpterCore

final class ClientAttributionTests: XCTestCase {
    func testNewAttributionRoundTripAndHistoricalMissingFields() throws {
        let raw = #"{"id":"new","kind":"client","timestamp":1,"statusCode":200,"durationMS":1,"failover":false,"clientKind":"deepseek_harness","clientVariant":"unknown","agentRole":"subagent","requestKind":"compaction","clientAttribution":{"version":1,"evidence":[{"field":"requestKind","source":"native","rule":"deepseek-compact-v1","value":"compaction"}],"conflicts":["requestKind"],"issues":[]}}"#
        let event = try JSONDecoder().decode(RuntimeEvent.self, from: Data(raw.utf8))
        XCTAssertEqual(event.requestKind, "compaction")
        XCTAssertEqual(try JSONDecoder().decode(RuntimeEvent.self, from: JSONEncoder().encode(event)), event)
        XCTAssertTrue(RuntimeEventPresentation.clientAttribution(event.clientAttribution).contains("原生标记"))
        XCTAssertTrue(RuntimeEventPresentation.clientAttribution(event.clientAttribution).contains("信号冲突"))
        var old = event
        old.requestKind = nil
        old.clientAttribution = nil
        let encoded = try JSONEncoder().encode(old)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertNil(object["requestKind"])
        XCTAssertNil(object["clientAttribution"])
        XCTAssertEqual(RuntimeEventPresentation.observedRequestKind(old.requestKind), "未记录")
        XCTAssertEqual(RuntimeEventPresentation.observedRequestKind("unknown"), "未识别")
        XCTAssertEqual(RuntimeEventPresentation.clientVariant("unknown"), "未识别")
        XCTAssertEqual(RuntimeEventPresentation.agentRole("subagent"), "子代理")
    }
}
