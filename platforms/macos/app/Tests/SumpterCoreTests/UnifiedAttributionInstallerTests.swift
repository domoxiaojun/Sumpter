import Foundation
import XCTest
@testable import SumpterApp

final class UnifiedAttributionInstallerTests: XCTestCase {
    func testStatusContractRejectsEmptyAndMalformedOutput() throws {
        let output = #"[{"client":"claude","status":"broken","rc":"/tmp/a b/.zshrc","shell":"zsh","canRestore":true}]"#
        let status = try XCTUnwrap(UnifiedAttributionInstaller.parseStatus(output).first)
        XCTAssertEqual(status.title, "配置不完整，需要修复")
        XCTAssertEqual(status.rc, "/tmp/a b/.zshrc")
        XCTAssertTrue(status.canRestore)
        XCTAssertThrowsError(try UnifiedAttributionInstaller.parseStatus("[]"))
        XCTAssertThrowsError(try UnifiedAttributionInstaller.parseStatus("not JSON"))
    }

    func testBundledInstallerRoundTripWithIsolatedHome() async throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("sumpter attribution ' \(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }
        let env = ["HOME": home.path, "XDG_DATA_HOME": home.appendingPathComponent("data").path, "PATH": "/usr/bin:/bin"]
        let initial = try await UnifiedAttributionInstaller.run(.status, client: "all", shell: "zsh", environment: env)
        XCTAssertEqual(try UnifiedAttributionInstaller.parseStatus(initial).map(\.status), ["absent", "absent", "absent"])
        _ = try await UnifiedAttributionInstaller.run(.install, client: "claude", shell: "zsh", environment: env)
        let installed = try await UnifiedAttributionInstaller.run(.status, client: "claude", shell: "zsh", environment: env)
        XCTAssertEqual(try UnifiedAttributionInstaller.parseStatus(installed).first?.status, "installed")
        let rc = home.appendingPathComponent(".zshrc")
        let text = try String(contentsOf: rc, encoding: .utf8)
        try (text + "# user edit\n").write(to: rc, atomically: true, encoding: .utf8)
        _ = try await UnifiedAttributionInstaller.run(.restore, client: "all", shell: "zsh", environment: env)
        XCTAssertEqual(try String(contentsOf: rc, encoding: .utf8), "# user edit\n")
        let restored = try await UnifiedAttributionInstaller.run(.status, client: "claude", shell: "zsh", environment: env)
        XCTAssertEqual(try UnifiedAttributionInstaller.parseStatus(restored).first?.canRestore, false)
    }
}
