import Darwin
import Foundation

enum ClaudeAttributionScriptAction: String, Sendable {
    case status
    case install
    case restore
    case uninstall
}

struct ClaudeAttributionScriptExecution: Equatable, Sendable {
    let action: ClaudeAttributionScriptAction
    let exitCode: Int32
    let standardOutput: String
    let standardError: String

    var succeeded: Bool { exitCode == 0 }

    var combinedOutput: String {
        [standardOutput, standardError]
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: "\n")
    }
}

struct ClaudeAttributionInstallationStatus: Equatable, Sendable {
    enum Condition: Equatable, Sendable {
        case installed
        case notInstalled
        case needsRepair
        case blockedBySettings
    }

    let shell: String
    let rcPath: String
    let markerInstalled: Bool
    let snippetExists: Bool
    let settingsConflict: Bool
    let backupCount: Int

    var condition: Condition {
        if settingsConflict { return .blockedBySettings }
        if markerInstalled, snippetExists { return .installed }
        if markerInstalled { return .needsRepair }
        return .notInstalled
    }

    var canRemove: Bool { markerInstalled || snippetExists }
    var canRestore: Bool { backupCount > 0 }
}

enum ClaudeAttributionInstallerError: LocalizedError, Equatable {
    case scriptMissing(String)
    case invalidStatusOutput

    var errorDescription: String? {
        switch self {
        case let .scriptMissing(path):
            "找不到项目归因配置器：\(path)"
        case .invalidStatusOutput:
            "配置器返回了无法识别的状态，请在高级说明中复制 status 命令手动检查。"
        }
    }
}

enum ClaudeAttributionInstaller {
    static func run(
        scriptURL: URL,
        action: ClaudeAttributionScriptAction
    ) async throws -> ClaudeAttributionScriptExecution {
        let scriptPath = scriptURL.path
        guard FileManager.default.fileExists(atPath: scriptPath) else {
            throw ClaudeAttributionInstallerError.scriptMissing(scriptPath)
        }

        let inheritedEnvironment = ProcessInfo.processInfo.environment
        let loginShell = detectedLoginShell()
        return try await Task.detached(priority: .userInitiated) {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/bash")
            // 参数逐项交给 Process，不拼 shell 命令字符串，路径中的空格也不会被重新解释。
            process.arguments = [scriptPath, action.rawValue]

            var environment = inheritedEnvironment
            if environment["SHELL"]?.isEmpty != false {
                environment["SHELL"] = loginShell
            }
            process.environment = environment

            let outputPipe = Pipe()
            let errorPipe = Pipe()
            process.standardOutput = outputPipe
            process.standardError = errorPipe

            try process.run()
            process.waitUntilExit()

            let output = String(
                decoding: outputPipe.fileHandleForReading.readDataToEndOfFile(),
                as: UTF8.self
            )
            let error = String(
                decoding: errorPipe.fileHandleForReading.readDataToEndOfFile(),
                as: UTF8.self
            )
            return ClaudeAttributionScriptExecution(
                action: action,
                exitCode: process.terminationStatus,
                standardOutput: output,
                standardError: error
            )
        }.value
    }

    static func parseStatus(_ output: String) throws -> ClaudeAttributionInstallationStatus {
        let lines = output.split(whereSeparator: \.isNewline).map(String.init)

        func value(after prefix: String) -> String? {
            guard let line = lines.first(where: { $0.hasPrefix(prefix) }) else { return nil }
            return String(line.dropFirst(prefix.count))
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }

        guard let shell = value(after: "shell:"),
              let rc = value(after: "rc:"),
              let marker = value(after: "rc 内标记块:"),
              let snippet = value(after: "snippet:"),
              let settings = value(after: "settings.json 键:")
        else {
            throw ClaudeAttributionInstallerError.invalidStatusOutput
        }

        return ClaudeAttributionInstallationStatus(
            shell: shell,
            rcPath: rc.replacingOccurrences(of: " (不存在)", with: ""),
            markerInstalled: marker.contains("已安装"),
            snippetExists: !snippet.contains("(不存在)"),
            settingsConflict: settings.contains("存在("),
            backupCount: lines.filter { $0.contains(".kekulv-bak-") }.count
        )
    }

    private static func detectedLoginShell() -> String {
        guard let entry = getpwuid(getuid()), let shell = entry.pointee.pw_shell else {
            return "/bin/zsh"
        }
        let value = String(cString: shell)
        return value.isEmpty ? "/bin/zsh" : value
    }
}
