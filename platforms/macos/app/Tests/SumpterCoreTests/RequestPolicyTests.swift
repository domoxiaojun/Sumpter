import Foundation
import XCTest
@testable import SumpterCore

final class RequestPolicyTests: XCTestCase {
    func testOldConfigKeepsDefaultsAndScopedProfilesRoundTrip() throws {
        let legacy = try JSONDecoder().decode(AppConfig.self, from: Data(#"{"schemaVersion":7}"#.utf8))
        XCTAssertEqual(legacy.requestPolicy, RequestPolicy())
        XCTAssertEqual(legacy.requestPolicy.maxConsecutiveNoResponse, 3)
        XCTAssertEqual(legacy.requestPolicy.noResponseCooldownSeconds, 300)
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

    func testNoResponseProtectionDisableAndTerminalFailureRoundTrip() throws {
        let policy = try JSONDecoder().decode(RequestPolicy.self, from: Data(#"{"maxConsecutiveNoResponse":0,"noResponseCooldownSeconds":60}"#.utf8))
        XCTAssertEqual(policy.maxConsecutiveNoResponse, 0)
        XCTAssertEqual(policy.noResponseCooldownSeconds, 60)
        XCTAssertEqual(try JSONDecoder().decode(RequestPolicy.self, from: JSONEncoder().encode(policy)), policy)
        let data = Data(#"{"id":"r","timestamp":1,"kind":"client","statusCode":422,"durationMS":5,"failover":false,"phase":"completed","outcome":"failed","failureKind":"no_response_retry_exhausted","failurePhase":"before_response","requestTrace":{"steps":[{"action":"stop","detail":"no_response_retry_exhausted","atMs":5}],"omitted":0}}"#.utf8)
        let event = try JSONDecoder().decode(RuntimeEvent.self, from: data)
        XCTAssertEqual(event.failureKind, .noResponseRetryExhausted)
        XCTAssertEqual(event.outcome, .failed)
        XCTAssertEqual(event.requestTrace?.steps.first?.reason, "连续未收到响应，已停止向该入口重试")
        XCTAssertEqual(RuntimeEventPresentation.failureKindDisplay(.noResponseRetryExhausted), "连续无响应保护")
        XCTAssertEqual(try JSONDecoder().decode(RuntimeEvent.self, from: JSONEncoder().encode(event)).failureKind, event.failureKind)
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
