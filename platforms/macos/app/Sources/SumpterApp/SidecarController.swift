import Foundation

/// Sumpter sidecar 进程管理:spawn、stdout 握手、退出监视、SIGTERM 停止、孤儿收尸。
///
/// 生命周期约定(与 Sumpter sidecar 对齐):
/// - spawn 时保持 stdin 管道写端 —— app 退出/崩溃即 EOF,sidecar 随之自杀,不留孤儿;
/// - sidecar 就绪后向 stdout 写一行握手 JSON(adminPort/proxyPort/pid/generation);
/// - 正常停止 = SIGTERM → 限时等待 → SIGKILL 兜底;
/// - 意外退出经 `onUnexpectedExit` 上报,由 AppModel 决定退避重启与 UI 状态。
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
                return "Sumpter sidecar 握手超时(10 秒内未就绪)"
            case .handshakeInvalid(let line):
                return "Sumpter sidecar 握手输出无法解析:\(line)"
            }
        }
    }

    public private(set) var handshake: Handshake?
    /// 进程意外退出(非主动 stop)时回调退出码;AppModel 借此做退避重启与「进程异常」态。
    public var onUnexpectedExit: (@MainActor (Int32) -> Void)?

    private var process: Process?
    private var stdinPipe: Pipe?
    private var expectingExit = false

    public init() {}

    public var isRunning: Bool {
        process?.isRunning ?? false
    }

    /// 定位 Sumpter sidecar:环境变量 SUMPTERD_PATH(开发用)→ bundle 辅助可执行(打包后)。
    public static func locateBinary() -> URL? {
        if let override = ProcessInfo.processInfo.environment["SUMPTERD_PATH"],
           !override.isEmpty {
            let url = URL(fileURLWithPath: override)
            if FileManager.default.isExecutableFile(atPath: url.path) {
                return url
            }
        }
        if let aux = Bundle.main.url(forAuxiliaryExecutable: "sumpterd"),
           FileManager.default.isExecutableFile(atPath: aux.path) {
            return aux
        }
        return nil
    }

    /// 启动 sidecar 并等握手。`configDir` 传给 --config-dir;stderr 追加到 sumpterd.stderr.log。
    @discardableResult
    public func start(configDir: URL) async throws -> Handshake {
        guard process == nil || process?.isRunning != true else {
            throw SidecarError.alreadyRunning
        }
        guard let binary = Self.locateBinary() else {
            throw SidecarError.binaryNotFound
        }

        Self.reapOrphan(pidFile: configDir.appendingPathComponent("sumpterd.pid"))

        let child = Process()
        child.executableURL = binary
        child.arguments = ["--config-dir", configDir.path]

        let stdin = Pipe()
        let stdout = Pipe()
        child.standardInput = stdin
        child.standardOutput = stdout
        child.standardError = Self.stderrHandle(configDir: configDir) ?? FileHandle.nullDevice

        child.terminationHandler = { [weak self] finished in
            let code = finished.terminationStatus
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.process = nil
                self.stdinPipe = nil
                self.handshake = nil
                if !self.expectingExit {
                    self.onUnexpectedExit?(code)
                }
                self.expectingExit = false
            }
        }

        do {
            try child.run()
        } catch {
            throw SidecarError.launchFailed(error.localizedDescription)
        }
        process = child
        stdinPipe = stdin // 持有写端:app 退出即 EOF。
        expectingExit = false

        do {
            let line = try await Self.readFirstLine(
                from: stdout.fileHandleForReading,
                timeoutSeconds: 10
            )
            guard let data = line.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode(Handshake.self, from: data),
                  decoded.event == "ready" else {
                await stop()
                throw SidecarError.handshakeInvalid(line)
            }
            handshake = decoded
            return decoded
        } catch let error as SidecarError {
            throw error
        } catch {
            await stop()
            throw SidecarError.handshakeTimeout
        }
    }

    /// SIGTERM → 最多等 3 秒 → SIGKILL。
    public func stop() async {
        guard let child = process else {
            return
        }
        expectingExit = true
        if child.isRunning {
            child.terminate()
        }
        for _ in 0..<30 where child.isRunning {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        if child.isRunning {
            kill(child.processIdentifier, SIGKILL)
        }
        process = nil
        stdinPipe = nil
        handshake = nil
    }

    /// 启动前收尸:上一个实例(app 崩溃遗留)按 pid 文件 SIGTERM。
    /// pid 文件由 Sumpter sidecar 正常退出时清理,存在即疑似孤儿。
    public static func reapOrphan(pidFile: URL) {
        guard let text = try? String(contentsOf: pidFile, encoding: .utf8),
              let pid = Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)),
              pid > 1 else {
            return
        }
        if kill(pid, 0) == 0 {
            kill(pid, SIGTERM)
        }
        try? FileManager.default.removeItem(at: pidFile)
    }

    private static func stderrHandle(configDir: URL) -> FileHandle? {
        let url = configDir.appendingPathComponent("sumpterd.stderr.log")
        if !FileManager.default.fileExists(atPath: url.path) {
            FileManager.default.createFile(atPath: url.path, contents: nil)
        }
        guard let handle = try? FileHandle(forWritingTo: url) else {
            return nil
        }
        _ = try? handle.seekToEnd()
        return handle
    }

    private static func readFirstLine(
        from handle: FileHandle,
        timeoutSeconds: UInt64
    ) async throws -> String {
        try await withThrowingTaskGroup(of: String.self) { group in
            group.addTask {
                for try await line in handle.bytes.lines {
                    return line
                }
                throw SidecarError.handshakeInvalid("(stdout 提前关闭)")
            }
            group.addTask {
                try await Task.sleep(nanoseconds: timeoutSeconds * 1_000_000_000)
                throw SidecarError.handshakeTimeout
            }
            guard let first = try await group.next() else {
                throw SidecarError.handshakeTimeout
            }
            group.cancelAll()
            return first
        }
    }
}
