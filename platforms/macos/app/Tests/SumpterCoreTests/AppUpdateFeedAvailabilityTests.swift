import XCTest
@testable import SumpterApp

final class AppUpdateFeedAvailabilityTests: XCTestCase {
    func testEmptyFeedAndKeyAreMissingFeed() {
        XCTAssertEqual(
            AppUpdateFeedAvailability.resolve(feedURL: nil, publicKey: nil),
            .missingFeed
        )
        XCTAssertEqual(
            AppUpdateFeedAvailability.resolve(feedURL: "  ", publicKey: ""),
            .missingFeed
        )
        XCTAssertFalse(AppUpdateFeedAvailability.resolve(feedURL: nil, publicKey: nil).canCheck)
        XCTAssertNotNil(AppUpdateFeedAvailability.resolve(feedURL: nil, publicKey: nil).userMessage)
    }

    func testCompleteFeedIsReady() {
        let availability = AppUpdateFeedAvailability.resolve(
            feedURL: " https://raw.githubusercontent.com/domoxiaojun/sumpter/macos-updates/macos/appcast.xml ",
            publicKey: "examplePublicKey"
        )
        XCTAssertEqual(availability, .ready)
        XCTAssertTrue(availability.canCheck)
        XCTAssertNil(availability.userMessage)
    }

    func testPartialOrInvalidFeedIsIncomplete() {
        XCTAssertEqual(
            AppUpdateFeedAvailability.resolve(
                feedURL: "https://example.com/appcast.xml",
                publicKey: nil
            ),
            .incomplete
        )
        XCTAssertEqual(
            AppUpdateFeedAvailability.resolve(feedURL: nil, publicKey: "only-key"),
            .incomplete
        )
        XCTAssertEqual(
            AppUpdateFeedAvailability.resolve(feedURL: "   ", publicKey: "only-key"),
            .incomplete
        )
        XCTAssertFalse(
            AppUpdateFeedAvailability.resolve(
                feedURL: "https://example.com/appcast.xml",
                publicKey: "  "
            ).canCheck
        )
    }
}
