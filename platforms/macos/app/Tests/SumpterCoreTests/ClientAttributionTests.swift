import XCTest
@testable import SumpterCore

final class ClientAttributionTests: XCTestCase {
    func testWorkspaceIdentityRoundTripKeepsHistoricalFieldsOptional() throws {
        let raw = #"{"workspace":".../apps/api","workspaceIdentity":{"version":2,"key":"synthetic","name":"api","scoped":true},"sourceId":"host-a","collectorVersion":"2.0.0-test"}"#
        let metadata = try JSONDecoder().decode(ClientDeclaredMetadata.self, from: Data(raw.utf8))
        XCTAssertEqual(metadata.workspaceIdentity?.version, 2)
        XCTAssertEqual(metadata.workspaceIdentity?.name, "api")
        XCTAssertEqual(try JSONDecoder().decode(ClientDeclaredMetadata.self, from: JSONEncoder().encode(metadata)), metadata)
        let native = try JSONDecoder().decode(CodexMetadata.self, from: Data(raw.utf8))
        XCTAssertEqual(native.workspaceIdentity, metadata.workspaceIdentity)
        XCTAssertEqual(try JSONDecoder().decode(CodexMetadata.self, from: JSONEncoder().encode(native)), native)
        let old = try JSONDecoder().decode(ClientDeclaredMetadata.self, from: Data(#"{"workspace":".../apps/api"}"#.utf8))
        XCTAssertNil(old.workspaceIdentity)
        XCTAssertNil(old.sourceId)
    }

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
