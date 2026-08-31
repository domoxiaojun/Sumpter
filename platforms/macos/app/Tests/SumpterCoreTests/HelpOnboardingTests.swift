import XCTest
@testable import SumpterApp

final class HelpOnboardingTests: XCTestCase {
    func testSixStateOnboardingContract() {
        XCTAssertEqual(
            resolveHelpOnboardingState(
                isRunning: false,
                hasConfig: false,
                hasMapping: false,
                clientRequests: 0,
                clientSuccesses: 0,
                clientFailures: 0
            ),
            .notStarted
        )
        XCTAssertEqual(
            resolveHelpOnboardingState(
                isRunning: true,
                hasConfig: false,
                hasMapping: false,
                clientRequests: 0,
                clientSuccesses: 0,
                clientFailures: 0
            ),
            .notConfigured
        )
        XCTAssertEqual(
            resolveHelpOnboardingState(
                isRunning: true,
                hasConfig: true,
                hasMapping: false,
                clientRequests: 0,
                clientSuccesses: 0,
                clientFailures: 0
            ),
            .noMapping
        )
        XCTAssertEqual(
            resolveHelpOnboardingState(
                isRunning: true,
                hasConfig: true,
                hasMapping: true,
                clientRequests: 0,
                clientSuccesses: 0,
                clientFailures: 0
            ),
            .clientNotConnected
        )
        XCTAssertEqual(
            resolveHelpOnboardingState(
                isRunning: true,
                hasConfig: true,
                hasMapping: true,
                clientRequests: 1,
                clientSuccesses: 0,
                clientFailures: 1
            ),
            .firstFailure
        )
        XCTAssertEqual(
            resolveHelpOnboardingState(
                isRunning: true,
                hasConfig: true,
                hasMapping: true,
                clientRequests: 2,
                clientSuccesses: 1,
                clientFailures: 1
            ),
            .firstSuccess
        )
    }
}

