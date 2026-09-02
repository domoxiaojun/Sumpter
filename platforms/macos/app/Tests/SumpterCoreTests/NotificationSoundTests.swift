import Foundation
import XCTest
@testable import SumpterApp

final class NotificationSoundTests: XCTestCase {
    func testSystemSoundOptionsComeFromFiles() {
        let options = SystemSoundCatalog.available()
        XCTAssertFalse(options.isEmpty, "macOS 应提供至少一个系统音效文件")
        let titles = options.map(\.title)
        XCTAssertEqual(titles, titles.sorted { $0.localizedStandardCompare($1) == .orderedAscending })
        for option in options {
            XCTAssertTrue(FileManager.default.fileExists(atPath: option.url.path))
            XCTAssertEqual(NotificationSoundPreference.system(fileName: option.fileName).systemSoundName, option.fileName)
        }
    }

    func testLegacySoundPreferenceMigratesByCurrentFileTitle() {
        guard let option = SystemSoundCatalog.available().first else {
            return
        }
        let defaults = UserDefaults.standard
        let previous = defaults.object(forKey: NotificationSoundPreference.defaultsKey)
        defer {
            if let previous {
                defaults.set(previous, forKey: NotificationSoundPreference.defaultsKey)
            } else {
                defaults.removeObject(forKey: NotificationSoundPreference.defaultsKey)
            }
        }

        defaults.set(option.title.lowercased(), forKey: NotificationSoundPreference.defaultsKey)
        XCTAssertEqual(NotificationSoundPreference.load(), .system(fileName: option.fileName))
    }
}
