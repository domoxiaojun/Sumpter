import Foundation
import XCTest
@testable import SumpterCore

final class RequestPolicyTests: XCTestCase {
    func testOldConfigKeepsDefaultsAndScopedProfilesRoundTrip() throws {
        let legacy = try JSONDecoder().decode(AppConfig.self, from: Data(#"{"schemaVersion":7}"#.utf8))
        XCTAssertEqual(legacy.requestPolicy, RequestPolicy())
        let oldObject = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(legacy)) as? [String: Any])
        XCTAssertNil(oldObject["requestPolicy"])
        let data = Data(#"{"schemaVersion":7,"requestPolicy":{"streamProtection":"prelude","maxTotalAttempts":6,"streamBufferMib":8,"endpoints":{"a":{"limits":{"maxConcurrent":2},"protocolModels":{"openai-responses":{"m":{"strictTools":false}}}}}}}"#.utf8)
        let config = try JSONDecoder().decode(AppConfig.self, from: data)
        XCTAssertEqual(config.requestPolicy.streamBufferMiB, 8)
        XCTAssertEqual(config.requestPolicy.responseWaitTimeoutSeconds, 0)
        XCTAssertEqual(config.requestPolicy.endpoints["a"]?.limits.maxQueued, 128)
        XCTAssertEqual(config.requestPolicy.endpoints["a"]?.protocolModels["openai-responses"]?["m"]?.strictTools, false)
        let roundTrip = try JSONDecoder().decode(AppConfig.self, from: JSONEncoder().encode(config))
        XCTAssertEqual(roundTrip.requestPolicy, config.requestPolicy)
    }

    func testLocalFailureAndTraceSurviveProjection() throws {
        let data = Data(#"{"id":"r","timestamp":1,"kind":"client","statusCode":429,"durationMS":5,"failover":false,"failureKind":"local_admission_rejected","requestTrace":{"steps":[{"action":"admission_rejected","detail":"queue_full","atMs":5,"durationMs":4}],"omitted":0}}"#.utf8)
        let event = try JSONDecoder().decode(RuntimeEvent.self, from: data)
        XCTAssertEqual(event.failureKind, .localAdmissionRejected)
        var projection = event
        projection.requestTrace = nil
        projection.detailsOmitted = true
        XCTAssertEqual(event.mergingProjection(projection).requestTrace, event.requestTrace)
        let roundTrip = try JSONDecoder().decode(RuntimeEvent.self, from: JSONEncoder().encode(event))
        XCTAssertEqual(roundTrip.requestTrace, event.requestTrace)
    }
}
