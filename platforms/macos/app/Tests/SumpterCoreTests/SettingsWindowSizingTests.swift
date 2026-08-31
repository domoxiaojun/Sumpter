import AppKit
import XCTest
@testable import SumpterApp

final class SettingsWindowSizingTests: XCTestCase {
    func testPreferredContentSizeFitsInsideVisibleFrame() {
        let size = SettingsWindowController.fittingContentSize(
            preferred: SettingsWindowController.preferredContentSize,
            visibleFrame: NSRect(x: 0, y: 0, width: 1_200, height: 720)
        )

        XCTAssertEqual(size.width, 1_168)
        XCTAssertEqual(size.height, 672)
    }

    func testPreferredContentSizeIsKeptOnLargeDisplay() {
        let size = SettingsWindowController.fittingContentSize(
            preferred: SettingsWindowController.preferredContentSize,
            visibleFrame: NSRect(x: 0, y: 0, width: 2_560, height: 1_400)
        )

        XCTAssertEqual(size, SettingsWindowController.preferredContentSize)
    }

    func testRestoredFrameIsMovedAndShrunkIntoVisibleFrame() {
        let visible = NSRect(x: -1_920, y: 40, width: 1_920, height: 1_040)
        let restored = NSRect(x: -2_400, y: -120, width: 2_200, height: 1_200)

        let result = SettingsWindowController.constrainedFrame(restored, visibleFrame: visible)

        XCTAssertEqual(result, visible)
    }
}
