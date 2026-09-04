import Darwin
import Foundation
import SumpterCore

enum GrokNotificationHooksError: Error, LocalizedError {
    case grokHomeMissing(String)

    var errorDescription: String? {
        switch self {
        case .grokHomeMissing(let path):
            return "GROK_HOME 不是可用目录：\(path)"
        }
    }
}

/// 把 Sumpter 通知 hook 写进 `$GROK_HOME/hooks/sumpter-notify.json`（默认 `~/.grok`）。
/// 文件由 Sumpter 独占；卸载时删除该 JSON 和脚本，不改 `config.toml`。
enum GrokNotificationHooks {
    static var marker: String { GrokNotifyScript.fileName }

    static func resolvedHomePath(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String {
        locations(environment: environment).home.path
    }

    static func resolvedHooksJSONPath(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String {
        locations(environment: environment).hooksJSON.path
    }

    static func isEnabled(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> Bool {
        !enabledArguments(environment: environment).isEmpty
    }

    static func enabledArguments(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> Set<String> {
        let url = locations(environment: environment).hooksJSON
        guard FileManager.default.fileExists(atPath: url.path),
              let data = try? Data(contentsOf: url),
              let object = try? JSONSerialization.jsonObject(with: data),
              let root = object as? [String: Any],
              GrokNotifyHookFile.containsCommand(matching: marker, in: root) else {
            return []
        }
        return Set(GrokNotifyHookFile.notificationEvents)
    }

    static func setEnabled(_ enabled: Bool, port: Int) throws {
        try setEnabled(
            enabled,
            port: port,
            environment: ProcessInfo.processInfo.environment
        )
    }

    static func setEnabled(
        _ enabled: Bool,
        port: Int,
        environment: [String: String]
    ) throws {
        let locations = try writableLocations(environment: environment)
        if enabled {
            let token = try ControlTokenStore.ensureToken(at: SumpterPaths.controlTokenURL())
            let command = GrokNotifyScript.command(path: locations.script.path)
            let script = GrokNotifyScript.content(port: port, token: token)
            let json = try GrokNotifyHookFile.encodeRoot(GrokNotifyHookFile.root(command: command))
            try writeAtomically(Data(script.utf8), to: locations.script, permissions: 0o700)
            try writeAtomically(json, to: locations.hooksJSON, permissions: 0o600)
        } else {
            try removeIfPresent(locations.hooksJSON)
            try removeIfPresent(locations.script)
        }
    }

    static func rewriteScriptIfEnabled(port: Int) throws {
        guard isEnabled() else { return }
        try setEnabled(true, port: port)
    }

    private struct Locations {
        let home: URL
        let hooksDirectory: URL
        let hooksJSON: URL
        let script: URL
    }

    private static func locations(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) -> Locations {
        let raw = environment["GROK_HOME"]?.trimmingCharacters(in: .whitespacesAndNewlines)
        let home: URL
        if let raw, !raw.isEmpty {
            home = URL(fileURLWithPath: raw, isDirectory: true).standardizedFileURL
        } else {
            home = fileManager.homeDirectoryForCurrentUser
                .appendingPathComponent(".grok", isDirectory: true)
        }
        let hooksDirectory = home.appendingPathComponent("hooks", isDirectory: true)
        return Locations(
            home: home,
            hooksDirectory: hooksDirectory,
            hooksJSON: hooksDirectory.appendingPathComponent(GrokNotifyHookFile.fileName),
            script: home.appendingPathComponent(GrokNotifyScript.fileName)
        )
    }

    private static func writableLocations(environment: [String: String]) throws -> Locations {
        let found = locations(environment: environment)
        let fileManager = FileManager.default
        if let raw = environment["GROK_HOME"]?.trimmingCharacters(in: .whitespacesAndNewlines),
           !raw.isEmpty {
            var isDirectory: ObjCBool = false
            if fileManager.fileExists(atPath: found.home.path, isDirectory: &isDirectory) {
                guard isDirectory.boolValue else {
                    throw GrokNotificationHooksError.grokHomeMissing(found.home.path)
                }
            }
        }
        try fileManager.createDirectory(at: found.hooksDirectory, withIntermediateDirectories: true)
        return found
    }

    private static func removeIfPresent(_ url: URL) throws {
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
    }

    private static func writeAtomically(_ data: Data, to url: URL, permissions: Int) throws {
        let fileManager = FileManager.default
        let directory = url.deletingLastPathComponent()
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        let temporary = directory.appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        var renamed = false
        defer {
            if !renamed { try? fileManager.removeItem(at: temporary) }
        }
        try data.write(to: temporary, options: .withoutOverwriting)
        try fileManager.setAttributes([.posixPermissions: permissions], ofItemAtPath: temporary.path)
        let handle = try FileHandle(forWritingTo: temporary)
        try handle.synchronize()
        try handle.close()
        let result = temporary.path.withCString { source in
            url.path.withCString { destination in Darwin.rename(source, destination) }
        }
        guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        renamed = true
        let directoryDescriptor = Darwin.open(directory.path, O_RDONLY)
        if directoryDescriptor >= 0 {
            _ = Darwin.fsync(directoryDescriptor)
            Darwin.close(directoryDescriptor)
        }
    }
}
