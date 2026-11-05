import Foundation
import XCTest
@testable import SumpterCore

final class InboundTokenTests: XCTestCase {
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
