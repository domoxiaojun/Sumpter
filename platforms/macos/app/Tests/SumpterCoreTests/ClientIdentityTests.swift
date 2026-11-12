import Foundation
import XCTest
@testable import SumpterCore

final class ClientIdentityTests: XCTestCase {
    func testOptionalConfigRoundTripAndSharedCatalog() throws {
        var listener = try JSONDecoder().decode(ListenerConfig.self, from: Data(#"{"port":57878}"#.utf8))
        XCTAssertNil(listener.clientIdentity)
        var settings = ClientIdentitySettings()
        var binding = ClientBinding()
        binding.client = "custom-sdk"
        binding.variant = "work_cca"
        settings.tokenBindings["legacy"] = binding
        settings.reportingAliases = ["custom-sdk":"codex"]
        listener.clientIdentity = settings
        try settings.validate()
        XCTAssertEqual(try JSONDecoder().decode(ListenerConfig.self, from: JSONEncoder().encode(listener)), listener)
        XCTAssertEqual(Set(ClientRegistry.catalog.map(\.id)).count, ClientRegistry.catalog.count)
    }
    func testMappingsAndRelayValidationAreIndependent() throws {
        XCTAssertEqual(try ClientIdentitySettings.parseAliases("custom-sdk=codex"), ["custom-sdk":"codex"])
        XCTAssertThrowsError(try ClientIdentitySettings.parseAliases("name=codex\nname=pi"))
        let endpoint = "Endpoint-" + String(repeating:"a",count:80)
        XCTAssertEqual(try ClientIdentitySettings.parseAliases(endpoint + "=home",catalog:false),[endpoint:"home"])
        var settings = ClientIdentitySettings()
        settings.reportingAliases = ["same":"invalid"]
        settings.uaAliases = ["same":"codex"]
        XCTAssertThrowsError(try settings.validate())
    }
    func testHistoricalAttributionStillDecodesWithoutIdentity() throws {
        let observation = try JSONDecoder().decode(ClientAttribution.self,from:Data(#"{"version":1,"evidence":[],"conflicts":[],"issues":[]}"#.utf8))
        XCTAssertNil(observation.identity)
    }
}
