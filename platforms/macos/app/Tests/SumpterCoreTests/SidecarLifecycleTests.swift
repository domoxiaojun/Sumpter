import XCTest
@testable import SumpterApp

@MainActor
final class SidecarLifecycleTests: XCTestCase {
    private func temporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("sumpter-sidecar-tests-\(UUID())")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }

    func testHandshakeTimeoutDoesNotWaitForPipeEOF() async throws {
        let pipe = Pipe()
        defer { try? pipe.fileHandleForWriting.close() }
        let began = ContinuousClock.now
        do {
            _ = try await SidecarController.readFirstLine(from: pipe.fileHandleForReading, timeoutSeconds: 0.1)
            XCTFail("应超时")
        } catch SidecarController.SidecarError.handshakeTimeout {
            XCTAssertLessThan(began.duration(to: .now), .seconds(1))
        }
    }

    func testHandshakeCancellationDoesNotWaitForPipeEOF() async throws {
        let pipe = Pipe()
        defer { try? pipe.fileHandleForWriting.close() }
        let task = Task { try await SidecarController.readFirstLine(from: pipe.fileHandleForReading, timeoutSeconds: 10) }
        try await Task.sleep(for: .milliseconds(30))
        task.cancel()
        let began = ContinuousClock.now
        do { _ = try await task.value; XCTFail("应取消") }
        catch is CancellationError { XCTAssertLessThan(began.duration(to: .now), .seconds(1)) }
    }

    func testHandshakeReadsSplitLine() async throws {
        let pipe = Pipe()
        let task = Task { try await SidecarController.readFirstLine(from: pipe.fileHandleForReading, timeoutSeconds: 1) }
        try pipe.fileHandleForWriting.write(contentsOf: Data("{\"event\":".utf8))
        try await Task.sleep(for: .milliseconds(30))
        try pipe.fileHandleForWriting.write(contentsOf: Data("\"ready\"}\n".utf8))
        let value = try await task.value
        XCTAssertEqual(value, "{\"event\":\"ready\"}")
        try pipe.fileHandleForWriting.close()
    }

    func testStopClosesStdinBeforeWaitingForProcess() async throws {
        let directory = try temporaryDirectory()
        let script = directory.appendingPathComponent("fake-sidecar")
        let contents = """
        #!/bin/sh
        trap '' TERM
        printf '{"event":"ready","pid":%s,"proxyPort":1,"adminPort":1,"generation":"test"}\\n' "$$"
        while IFS= read -r line; do :; done
        exit 0
        """
        try contents.write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        let sidecar = SidecarController(binaryURL: script)
        _ = try await sidecar.start(configDir: directory)
        let began = ContinuousClock.now
        await sidecar.stop()
        XCTAssertFalse(sidecar.isRunning)
        XCTAssertLessThan(began.duration(to: .now), .seconds(2))
    }

    func testHandshakeFailureReapsStartedChild() async throws {
        let directory = try temporaryDirectory()
        let script = directory.appendingPathComponent("silent-sidecar")
        try "#!/bin/sh\nwhile IFS= read -r line; do :; done\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        let sidecar = SidecarController(binaryURL: script, handshakeTimeoutSeconds: 0.1)
        do { _ = try await sidecar.start(configDir: directory); XCTFail("应超时") }
        catch SidecarController.SidecarError.handshakeTimeout { XCTAssertFalse(sidecar.isRunning) }
    }

    func testStalePIDAndMismatchedIdentityNeverSignalUnrelatedChild() throws {
        let directory = try temporaryDirectory()
        let child = Process()
        child.executableURL = URL(fileURLWithPath: "/bin/sleep")
        child.arguments = ["10"]
        try child.run()
        defer { if child.isRunning { child.terminate() }; child.waitUntilExit() }
        let pidFile = directory.appendingPathComponent("sumpterd.pid")
        try String(child.processIdentifier).write(to: pidFile, atomically: true, encoding: .utf8)
        SidecarController.reapOrphan(pidFile: pidFile, binary: child.executableURL)
        XCTAssertTrue(child.isRunning)
        let actual = try XCTUnwrap(SidecarController.processIdentity(pid: child.processIdentifier, configDir: directory))
        let stale = SidecarController.ProcessIdentity(pid: actual.pid, executable: actual.executable,
            startedSeconds: actual.startedSeconds - 1, startedMicroseconds: actual.startedMicroseconds,
            configDirectory: actual.configDirectory)
        try JSONEncoder().encode(stale).write(to: directory.appendingPathComponent("sumpterd.identity.json"))
        SidecarController.reapOrphan(pidFile: pidFile, binary: child.executableURL)
        XCTAssertTrue(child.isRunning)
    }
}
