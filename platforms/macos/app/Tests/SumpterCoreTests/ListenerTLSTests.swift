import Foundation
import XCTest
@testable import SumpterCore

final class ListenerTLSTests: XCTestCase {
    func testLegacyHTTPAndOptionalHTTPSRoundTrip() throws {
        var listener = try JSONDecoder().decode(ListenerConfig.self, from: Data(#"{"port":57878}"#.utf8))
        XCTAssertNil(listener.httpsPort)
        listener.tls = ListenerTLSConfig(enabled: true, certificatePath: "/tmp/cert.pem", privateKeyPath: "/tmp/key.pem")
        try listener.validateTLS()
        XCTAssertEqual(listener.port, 57878)
        XCTAssertEqual(listener.httpsPort, 57880)
        XCTAssertEqual(try JSONDecoder().decode(ListenerConfig.self, from: JSONEncoder().encode(listener)), listener)
        listener.tls?.port = listener.port
        XCTAssertThrowsError(try listener.validateTLS())
        listener.tls?.enabled = false
        XCTAssertNoThrow(try listener.validateTLS())
        XCTAssertNil(listener.httpsPort)
    }
}
