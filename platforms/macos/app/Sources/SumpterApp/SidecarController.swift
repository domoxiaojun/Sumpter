import Darwin
import Foundation

/// 管理 sidecar 握手、退出与孤儿进程；stdin 写端的关闭同时解除 daemon 的 EOF 监视。
@MainActor
public final class SidecarController {
    public struct Handshake: Decodable, Equatable, Sendable {
        public let event: String
        public let pid: Int32
        public let proxyPort: Int
        public let adminPort: Int
        public let generation: String
    }

    public enum SidecarError: Error, LocalizedError {
        case binaryNotFound
        case alreadyRunning
        case launchFailed(String)
        case handshakeTimeout
        case handshakeInvalid(String)

        public var errorDescription: String? {
            switch self {
            case .binaryNotFound:
                return "找不到 Sumpter sidecar 可执行文件(bundle 内缺失,且未设 SUMPTERD_PATH)"
            case .alreadyRunning:
                return "sidecar 已在运行"
            case .launchFailed(let message):
                return "Sumpter sidecar 启动失败:\(message)"
            case .handshakeTimeout:
                return "Sumpter sidecar 握手超时"
            case .handshakeInvalid(let line):
                return "Sumpter sidecar 握手输出无法解析:\(line)"
            }
        }
    }

    struct ProcessIdentity: Codable, Equatable {
        let pid: Int32
        let executable: String
        let startedSeconds: UInt64
        let startedMicroseconds: UInt64
        let configDirectory: String
    }

    public private(set) var handshake: Handshake?
    public var onUnexpectedExit: (@MainActor (Int32) -> Void)?

    private var process: Process?
    private var stdinPipe: Pipe?
    private var expectingExit = false
    private var launchID: UUID?
    private let binaryURL: URL?
    private let handshakeTimeoutSeconds: TimeInterval

    public init(binaryURL: URL? = nil, handshakeTimeoutSeconds: TimeInterval = 10) {
        self.binaryURL = binaryURL
        self.handshakeTimeoutSeconds = handshakeTimeoutSeconds
    }

    public var isRunning: Bool { process?.isRunning ?? false }

    public static func locateBinary() -> URL? {
        if let override = ProcessInfo.processInfo.environment["SUMPTERD_PATH"], !override.isEmpty {
            let url = URL(fileURLWithPath: override)
            if FileManager.default.isExecutableFile(atPath: url.path) { return url }
        }
        if let aux = Bundle.main.url(forAuxiliaryExecutable: "sumpterd"),
           FileManager.default.isExecutableFile(atPath: aux.path) { return aux }
        return nil
    }

    @discardableResult
    public func start(configDir: URL) async throws -> Handshake {
        guard !isRunning else { throw SidecarError.alreadyRunning }
        guard let binary = binaryURL ?? Self.locateBinary() else { throw SidecarError.binaryNotFound }
        Self.reapOrphan(pidFile: configDir.appendingPathComponent("sumpterd.pid"), binary: binary)

        let child = Process()
        child.executableURL = binary
        child.arguments = ["--config-dir", configDir.path]
        let stdin = Pipe()
        let stdout = Pipe()
        child.standardInput = stdin
        child.standardOutput = stdout
        child.standardError = Self.stderrHandle(configDir: configDir) ?? FileHandle.nullDevice
        let identity = UUID()
        child.terminationHandler = { [weak self] finished in
            let code = finished.terminationStatus
            Task { @MainActor [weak self] in
                guard let self, self.launchID == identity else { return }
                self.process = nil
                self.stdinPipe = nil
                self.handshake = nil
                self.launchID = nil
                if !self.expectingExit { self.onUnexpectedExit?(code) }
                self.expectingExit = false
            }
        }
        do { try child.run() }
        catch { throw SidecarError.launchFailed(error.localizedDescription) }
        process = child
        launchID = identity
        stdinPipe = stdin
        expectingExit = false

        do {
            let line = try await Self.readFirstLine(from: stdout.fileHandleForReading, timeoutSeconds: handshakeTimeoutSeconds)
            try Task.checkCancellation()
            guard launchID == identity, child.isRunning,
                  let data = line.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode(Handshake.self, from: data),
                  decoded.event == "ready", decoded.pid == child.processIdentifier else {
                throw SidecarError.handshakeInvalid(line)
            }
            if let record = Self.processIdentity(pid: decoded.pid, configDir: configDir),
               record.executable == binary.resolvingSymlinksInPath().path {
                let url = configDir.appendingPathComponent("sumpterd.identity.json")
                try JSONEncoder().encode(record).write(to: url, options: [.atomic])
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
            }
            handshake = decoded
            return decoded
        } catch {
            if launchID == identity { await stop() }
            throw error
        }
    }

    /// 先关闭 stdin，使 Tokio 的阻塞 stdin 读也能退出；强杀仅用于真实挂死。
    public func stop() async {
        guard let child = process else { return }
        let identity = launchID
        expectingExit = true
        try? stdinPipe?.fileHandleForWriting.close()
        stdinPipe = nil
        if child.isRunning { child.terminate() }
        for _ in 0..<30 where child.isRunning {
            // 即使调用者已取消，也等待进程回收，不能让取消把宽限期变成忙循环。
            await Task.detached { try? await Task.sleep(nanoseconds: 100_000_000) }.value
        }
        if child.isRunning { kill(child.processIdentifier, SIGKILL) }
        if launchID == identity {
            process = nil
            handshake = nil
            launchID = nil
            expectingExit = false
        }
    }

    static func processIdentity(pid: Int32, configDir: URL) -> ProcessIdentity? {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
        var path = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        guard proc_pidpath(pid, &path, UInt32(path.count)) > 0 else { return nil }
        return ProcessIdentity(
            pid: pid,
            executable: URL(fileURLWithPath: String(cString: path)).resolvingSymlinksInPath().path,
            startedSeconds: info.pbi_start_tvsec,
            startedMicroseconds: info.pbi_start_tvusec,
            configDirectory: configDir.resolvingSymlinksInPath().path
        )
    }

    /// 旧的纯 PID 文件不是身份凭据。仅回收同一配置目录、可执行文件和启动时刻的孤儿。
    public static func reapOrphan(pidFile: URL, binary: URL? = nil) {
        let directory = pidFile.deletingLastPathComponent()
        let identityURL = directory.appendingPathComponent("sumpterd.identity.json")
        guard let binary = binary ?? locateBinary(),
              let text = try? String(contentsOf: pidFile, encoding: .utf8),
              let pid = Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)), pid > 1,
              let data = try? Data(contentsOf: identityURL),
              let recorded = try? JSONDecoder().decode(ProcessIdentity.self, from: data),
              recorded.pid == pid,
              recorded.executable == binary.resolvingSymlinksInPath().path,
              let current = processIdentity(pid: pid, configDir: directory), current == recorded else { return }
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size,
              info.pbi_ppid == 1,
              info.pbi_start_tvsec == recorded.startedSeconds,
              info.pbi_start_tvusec == recorded.startedMicroseconds else { return }
        if kill(pid, SIGTERM) == 0 {
            try? FileManager.default.removeItem(at: pidFile)
            try? FileManager.default.removeItem(at: identityURL)
        }
    }

    private static func stderrHandle(configDir: URL) -> FileHandle? {
        let url = configDir.appendingPathComponent("sumpterd.stderr.log")
        if !FileManager.default.fileExists(atPath: url.path) { FileManager.default.createFile(atPath: url.path, contents: nil) }
        guard let handle = try? FileHandle(forWritingTo: url) else { return nil }
        _ = try? handle.seekToEnd()
        return handle
    }

    /// 非阻塞读允许取消和超时独立于子进程是否继续持有 stdout。
    static func readFirstLine(from handle: FileHandle, timeoutSeconds: TimeInterval) async throws -> String {
        let descriptor = handle.fileDescriptor
        let flags = fcntl(descriptor, F_GETFL)
        guard flags >= 0, fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) >= 0 else {
            throw SidecarError.handshakeInvalid("无法将 stdout 设为非阻塞")
        }
        defer { try? handle.close() }
        let deadline = ContinuousClock.now.advanced(by: .seconds(timeoutSeconds))
        var data = Data()
        var bytes = [UInt8](repeating: 0, count: 4096)
        while true {
            try Task.checkCancellation()
            let count = Darwin.read(descriptor, &bytes, bytes.count)
            if count > 0 {
                data.append(contentsOf: bytes.prefix(count))
                if let newline = data.firstIndex(of: 10) { return String(decoding: data[..<newline], as: UTF8.self) }
                guard data.count <= 64 * 1024 else { throw SidecarError.handshakeInvalid("握手行超过 64 KiB") }
            } else if count == 0 {
                throw SidecarError.handshakeInvalid("stdout 提前关闭")
            } else if errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR {
                throw SidecarError.handshakeInvalid("stdout 读取失败")
            }
            guard ContinuousClock.now < deadline else { throw SidecarError.handshakeTimeout }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
    }
}
