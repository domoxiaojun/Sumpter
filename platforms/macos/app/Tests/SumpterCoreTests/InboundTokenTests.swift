import Foundation
import XCTest
@testable import SumpterCore

final class InboundTokenTests: XCTestCase {
    func testUnifiedEditorPreservesDefaultCredentialAndStableIdentity() throws {
        var listener = ListenerConfig(authToken: "old-token", authTokens: [InboundTokenConfig(id: "work", name: "工作", token: "synthetic-work-token")])
        var entries = listener.editableInboundTokens
        XCTAssertEqual(entries.map(\.id), ["legacy", "work"])
        entries[0].name = "第一台电脑"
        entries[0].enabled = false
        try listener.setEditableInboundTokens(entries)
        XCTAssertEqual(listener.authToken, "old-token")
        XCTAssertEqual(listener.authTokenName, "第一台电脑")
        XCTAssertFalse(listener.authTokenEnabled)
        XCTAssertTrue(listener.hasInboundAuth)
        XCTAssertEqual(listener.authTokens.map(\.id), ["work"])
        entries[0].token = "replacement-token"
        try listener.setEditableInboundTokens(entries)
        XCTAssertEqual(listener.authToken, "replacement-token")
        try listener.setEditableInboundTokens(Array(entries.dropFirst()))
        XCTAssertTrue(listener.authToken.isEmpty)
        XCTAssertEqual(listener.authTokens.count, 1)
    }

    func testLegacyAndNamedTokenRoundTripWithoutSchemaUpgrade() throws {
        let legacy = try JSONDecoder().decode(ListenerConfig.self, from: Data(#"{"authToken":"old"}"#.utf8))
        XCTAssertEqual(legacy.authTokenName, "默认 Token")
        XCTAssertTrue(legacy.authTokens.isEmpty)
        var listener = legacy
        listener.authTokens = [InboundTokenConfig(id: "work", name: "工作电脑", token: "synthetic-work-token")]
        try listener.validateInboundTokens()
        XCTAssertEqual(try JSONDecoder().decode(ListenerConfig.self, from: JSONEncoder().encode(listener)), listener)
        listener.authToken = ""
        listener.authTokens[0].enabled = false
        XCTAssertTrue(listener.hasInboundAuth)
        listener.authTokens[0].token = ""
        XCTAssertThrowsError(try listener.validateInboundTokens())
        XCTAssertEqual(AppConfig.currentSchemaVersion, 7)
    }

    func testDuplicateAndReservedTokenIdentityIsRejected() throws {
        let entry = InboundTokenConfig(id: "work", name: "工作", token: "synthetic-work-token")
        var listener = ListenerConfig(authTokens: [entry, entry])
        XCTAssertThrowsError(try listener.validateInboundTokens())
        listener.authTokens = [entry]
        listener.authToken = entry.token
        XCTAssertThrowsError(try listener.validateInboundTokens())
        listener.authToken = ""
        listener.authTokens[0].id = "legacy"
        XCTAssertThrowsError(try listener.validateInboundTokens())
    }
}
