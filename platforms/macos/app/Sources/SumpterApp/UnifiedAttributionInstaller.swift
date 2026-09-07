import Darwin
import Foundation

struct UnifiedAttributionStatus: Decodable, Equatable, Sendable, Identifiable {
    let client: String
    let status: String
    let rc: String
    let shell: String
    let canRestore: Bool
    var id: String { client }
    var title: String {
        switch status {
        case "installed": "已安装"
        case "absent": "未安装"
        case "legacy": "已安装旧版配置，建议更新"
        case "broken": "配置不完整，需要修复"
        case "outdated": "已安装，需要更新"
        default: "未知状态，请重新检查"
        }
    }
}

enum UnifiedAttributionInstaller {
    static var scriptURL: URL? {
        Bundle.main.url(forResource: "client-attribution", withExtension: "mjs")
            ?? Bundle.module.url(forResource: "client-attribution", withExtension: "mjs")
    }

    static var loginShell: String {
        let value = ProcessInfo.processInfo.environment["SHELL"]
            ?? getpwuid(getuid()).flatMap { $0.pointee.pw_shell }.map { String(cString: $0) }
            ?? "/bin/zsh"
        return URL(fileURLWithPath: value).lastPathComponent
    }

    // Finder-launched apps do not inherit terminal PATH. Include Homebrew and common Node managers.
    static func executionEnvironment(_ inherited: [String: String]) -> [String: String] {
        var env = inherited
        let home = env["HOME"] ?? FileManager.default.homeDirectoryForCurrentUser.path
        let nvm = env["NVM_DIR"] ?? home + "/.nvm"
        let versions = (try? FileManager.default.contentsOfDirectory(atPath: nvm + "/versions/node")) ?? []
        let nodePaths = versions.sorted { $0.compare($1, options: .numeric) == .orderedDescending }
            .map { nvm + "/versions/node/" + $0 + "/bin" }
        env["PATH"] = ([env["PATH"] ?? "", "/opt/homebrew/bin", "/usr/local/bin",
                         home + "/.volta/bin", home + "/.local/share/mise/shims"]
                        + nodePaths + ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]).joined(separator: ":")
        return env
    }

    static func parseStatus(_ output: String) throws -> [UnifiedAttributionStatus] {
        let items = try JSONDecoder().decode([UnifiedAttributionStatus].self, from: Data(output.utf8))
        guard !items.isEmpty else { throw InstallerError.failed("配置器返回了空状态。") }
        return items
    }

    enum InstallerError: LocalizedError {
        case failed(String)
        var errorDescription: String? {
            switch self { case let .failed(message): message }
        }
    }

    static func run(
        _ action: ClaudeAttributionScriptAction, client: String, shell: String,
        scriptURL: URL? = Self.scriptURL, environment: [String: String] = ProcessInfo.processInfo.environment
    ) async throws -> String {
        guard let scriptURL else { throw InstallerError.failed("App 缺少归因配置器，请重新安装完整 App。") }
        let env = executionEnvironment(environment)
        return try await Task.detached(priority: .userInitiated) {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
            // Pass arguments directly; never source or evaluate the user's shell configuration.
            process.arguments = ["node", scriptURL.path, action.rawValue, client, "--shell", shell]
            process.environment = env
            process.currentDirectoryURL = URL(fileURLWithPath: env["HOME"] ?? NSHomeDirectory())
            process.standardInput = FileHandle.nullDevice
            let output = Pipe()
            process.standardOutput = output
            process.standardError = output
            try process.run()
            let timeout = DispatchWorkItem { if process.isRunning { process.terminate() } }
            DispatchQueue.global().asyncAfter(deadline: .now() + 30, execute: timeout)
            // Drain while running so verbose errors cannot fill the pipe and block the process.
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            timeout.cancel()
            let message = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            guard process.terminationStatus == 0 else {
                if message.contains("node: No such file") || message.contains("node: command not found") {
                    throw InstallerError.failed("未找到 Node.js。请安装 Node.js 18+ 后重新检查。")
                }
                throw InstallerError.failed(message.isEmpty ? "操作未完成或已超时，请重试。" : message)
            }
            return message
        }.value
    }
}
