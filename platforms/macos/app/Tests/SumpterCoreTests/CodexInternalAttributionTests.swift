import XCTest
@testable import SumpterCore

final class CodexInternalAttributionTests: XCTestCase {
    func testInternalRolesAndMetadataRoundTrip() throws {
        let data = Data(#"{"threadSource":"memory_consolidation","requestKind":"memory","isSubagent":true,"historyInitialization":"cold_resume","analyticsEnabled":false,"mcpAttribution":{"status":"complete","sourceCount":2}}"#.utf8)
        let metadata = try JSONDecoder().decode(CodexMetadata.self, from: data)
        XCTAssertEqual(RuntimeEventPresentation.codexRole(metadata), "记忆任务")
        let summary = RuntimeEventPresentation.codexSummary(metadata) ?? ""
        XCTAssertTrue(summary.contains("记忆提取"))
        XCTAssertTrue(summary.contains("历史初始化 cold_resume"))
        XCTAssertTrue(summary.contains("客户端分析 关闭"))
        XCTAssertTrue(summary.contains("MCP 归因 complete · 来源 2"))
        XCTAssertEqual(try JSONDecoder().decode(CodexMetadata.self, from: JSONEncoder().encode(metadata)), metadata)
    }

    func testConnectionRequestSummaryAndOldWire() throws {
        let data = Data(#"{"stage":"relay","requests":{"counts":{"prewarm":1,"turn":2},"controls":{},"recent":[{"sequence":3,"kind":"turn","turnId":"t2","responseId":"r2","terminal":"response.interrupted"}],"dropped":0,"unobservedFrames":1,"unassociatedEvents":2}}"#.utf8)
        let trace = try JSONDecoder().decode(WebSocketTrace.self, from: data)
        let summary = RuntimeEventPresentation.websocketSummary(trace)
        XCTAssertTrue(summary.contains("连接预热 1"))
        XCTAssertTrue(summary.contains("正式回合 2"))
        XCTAssertTrue(summary.contains("turn=t2 · response=r2 · response.interrupted"))
        XCTAssertNil(try JSONDecoder().decode(WebSocketTrace.self, from: Data(#"{"stage":"relay"}"#.utf8)).requests)
    }
}
