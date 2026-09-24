import Foundation
import XCTest
@testable import SumpterCore

final class UserAgentForceClientTests: XCTestCase {
    func testForceClientRoundTripsBesideProtocolUserAgent() throws {
        let data = Data(#"{"anthropic":{"mode":"auto","forceClient":true},"openai":{"mode":"override","value":"gateway/1"}}"#.utf8)
        let settings = try JSONDecoder().decode(UserAgentSettings.self, from: data)
        XCTAssertTrue(settings.anthropic.forceClient)
        XCTAssertFalse(settings.openai.forceClient)
        XCTAssertTrue(settings.summary.contains("Anthropic 强制 Claude Code"))

        let encoded = try JSONSerialization.jsonObject(with: JSONEncoder().encode(settings.validated())) as? [String: Any]
        let anthropic = encoded?["anthropic"] as? [String: Any]
        let openai = encoded?["openai"] as? [String: Any]
        XCTAssertEqual(anthropic?["forceClient"] as? Bool, true)
        XCTAssertNil(openai?["forceClient"])
    }

    func testGeminiRejectsForceClient() {
        let settings = UserAgentSettings(gemini: UserAgentRule(forceClient: true))
        XCTAssertThrowsError(try settings.validated())
    }

    func testLegacyEndpointForceClaudeCodeKeyIsIgnored() throws {
        let data = Data(#"{"id":"e","name":"e","baseURL":"https://e.invalid","protocol":"anthropic","forceClaudeCode":true}"#.utf8)
        let endpoint = try JSONDecoder().decode(Endpoint.self, from: data)
        let encoded = try JSONSerialization.jsonObject(with: JSONEncoder().encode(endpoint)) as? [String: Any]
        XCTAssertNil(encoded?["forceClaudeCode"])
    }
}
