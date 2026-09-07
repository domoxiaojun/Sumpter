import XCTest
@testable import SumpterCore

final class PiAttributionTests: XCTestCase {
    func testPiWireAndAttributionStates() throws {
        let client = try JSONDecoder().decode(ClientKind.self, from: Data("\"pi\"".utf8))
        XCTAssertEqual(client, .pi)
        XCTAssertEqual(PiAttributionHint.state(projects: []), .unknown)
        let known = ClaudeAttributionHint.ProjectRow(name: "project", projectSource: "workspace_local", clientKinds: ["pi"], attempts: 2)
        let missing = ClaudeAttributionHint.ProjectRow(name: "unidentified_project", projectSource: "missing_workspace_metadata", clientKinds: ["pi"], attempts: 1)
        XCTAssertEqual(PiAttributionHint.state(projects: [known]), .observed)
        XCTAssertEqual(PiAttributionHint.state(projects: [known, missing]), .unattributed)
        XCTAssertEqual(PiAttributionHint.state(projects: [.init(name: "unidentified_project", projectSource: "missing_workspace_metadata", clientKinds: ["codex"], attempts: 1)]), .unknown)
    }
}
