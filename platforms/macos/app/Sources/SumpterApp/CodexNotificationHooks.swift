import Darwin
import Foundation
import SumpterCore

enum CodexNotificationHookStatus: String, Sendable {
    case notConfigured
    case configured
    case verified
    case legacyConflict
    case writeFailed

    var title: String {
        switch self {
        case .notConfigured: "未配置"
        case .configured: "已配置"
        case .verified: "已验证"
        case .legacyConflict: "legacy 通知冲突"
        case .writeFailed: "写入失败"
        }
    }
}

enum CodexNotificationHooksError: Error, LocalizedError {
    case codexHomeMissing(String)
    case invalidHooks(CodexHookEditingError)
    case legacyConflict
    case invalidLegacyConfig

    var errorDescription: String? {
        switch self {
        case .codexHomeMissing(let path):
            return "CODEX_HOME 不是可用目录：\(path)"
        case .invalidHooks(let error):
            return error.localizedDescription
        case .legacyConflict:
            return "发现自定义或无法安全解析的 Codex legacy notify，请先手动移除后重试"
        case .invalidLegacyConfig:
            return "Codex config.toml 不是有效 UTF-8，拒绝修改"
        }
    }
}

enum CodexNotificationHooks {
    private static let verifiedDefaultsKey = "codexNotificationHookVerifiedPath"

    static var marker: String { CodexNotifyScript.fileName }

    static func isVerified() -> Bool {
        UserDefaults.standard.string(forKey: verifiedDefaultsKey) == resolvedHooksJSONPath()
    }

    static func markVerified() {
        UserDefaults.standard.set(resolvedHooksJSONPath(), forKey: verifiedDefaultsKey)
    }

    static func clearVerified() {
        UserDefaults.standard.removeObject(forKey: verifiedDefaultsKey)
    }

    static func currentStatus() -> CodexNotificationHookStatus {
        do {
            let locations = try locations()
            if let legacyURL = locations.configTOML,
               FileManager.default.fileExists(atPath: legacyURL.path) {
                guard let data = try? Data(contentsOf: legacyURL) else {
                    return .writeFailed
                }
                guard let text = String(data: data, encoding: .utf8) else {
                    return .writeFailed
                }
                if CodexLegacyNotifyEditing.removeKnownNotify(from: text).status == .conflict {
                    return .legacyConflict
                }
            }
            guard FileManager.default.fileExists(atPath: locations.hooksJSON.path) else {
                return .notConfigured
            }
            let root = try loadRoot(at: locations.hooksJSON)
            guard !enabledArguments(in: root).isEmpty else {
                return .notConfigured
            }
            return isVerified() ? .verified : .configured
        } catch {
            return .writeFailed
        }
    }

    static func resolvedHomePath(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String {
        if let path = try? locations(environment: environment).home.path {
            return path
        }
        let raw = environment["CODEX_HOME"]?.trimmingCharacters(in: .whitespacesAndNewlines)
        return raw.flatMap { $0.isEmpty ? nil : $0 } ?? "~/.codex"
    }

    static func resolvedHooksJSONPath(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String {
        if let path = try? locations(environment: environment).hooksJSON.path {
            return path
        }
        return URL(fileURLWithPath: resolvedHomePath(environment: environment), isDirectory: true)
            .appendingPathComponent("hooks.json").path
    }

    static func setEnabled(_ enabled: Bool, port: Int) throws {
        try setSelectedArguments(
            enabled ? Set(CodexHookEditing.notificationEvents) : [],
            port: port
        )
    }

    static func enabledArguments() -> Set<String> {
        guard let rootURL = try? locations().hooksJSON,
              FileManager.default.fileExists(atPath: rootURL.path),
              let root = try? loadRoot(at: rootURL) else {
            return []
        }
        return enabledArguments(in: root)
    }

    static func setSelectedArguments(_ arguments: Set<String>, port: Int) throws {
        let locations = try locations()
        var root = try loadRootIfPresent(at: locations.hooksJSON)
        let selected = Set(arguments).intersection(CodexHookEditing.notificationEvents)

        if !selected.isEmpty {
            let token = try ControlTokenStore.ensureToken(at: SumpterPaths.controlTokenURL())
            let command = CodexNotifyScript.command(path: locations.script.path)
            let entries = CodexHookEditing.notificationEvents.compactMap { event -> (event: String, command: String)? in
                selected.contains(event) ? (event: event, command: command) : nil
            }
            root = try CodexHookEditing.upsertCommands(entries, matching: marker, into: root)
            let script = CodexNotifyScript.content(port: port, token: token)

            // 先完成严格校验，再移除旧通知；这样 malformed hooks.json 不会
            // 造成旧通知已删、Sumpter Hook 却无法写入的意外状态。
            try removeKnownLegacyNotify(at: locations.configTOML)
            try writeAtomically(Data(script.utf8), to: locations.script, permissions: 0o700)
        } else {
            guard FileManager.default.fileExists(atPath: locations.hooksJSON.path) else {
                clearVerified()
                return
            }
            root = try CodexHookEditing.removingCommands(
                matching: marker,
                events: CodexHookEditing.notificationEvents,
                from: root
            )
        }

        try writeAtomically(
            CodexHookEditing.encodeRoot(root),
            to: locations.hooksJSON,
            permissions: 0o600
        )
        // 命令路径没变时 Codex 信任仍然有效。只有卸掉 Sumpter hook 才清验证。
        if selected.isEmpty {
            clearVerified()
        }
    }

    static func rewriteScriptIfEnabled(port: Int) throws {
        let locations = try locations()
        guard FileManager.default.fileExists(atPath: locations.hooksJSON.path) else { return }
        var root = try loadRoot(at: locations.hooksJSON)
        guard !enabledArguments(in: root).isEmpty else { return }
        // A previously enabled Sumpter hook may coexist with the old built-in
        // notify (for example after an app upgrade).  Reconciliation removes
        // only the known command; custom legacy values still raise conflict.
        try removeKnownLegacyNotify(at: locations.configTOML)
        let token = try ControlTokenStore.ensureToken(at: SumpterPaths.controlTokenURL())
        // 升级时把旧版只有 Stop 的配置补齐为完整的通知事件集合；仍只
        // 写入 Sumpter 自己的命令，用户 Hook 和未知字段保持不动。
        let command = CodexNotifyScript.command(path: locations.script.path)
        root = try CodexHookEditing.upsertCommands(
            CodexHookEditing.notificationEvents.map { (event: $0, command: command) },
            matching: marker,
            into: root
        )
        try writeAtomically(
            Data(CodexNotifyScript.content(port: port, token: token).utf8),
            to: locations.script,
            permissions: 0o700
        )
        try writeAtomically(
            CodexHookEditing.encodeRoot(root),
            to: locations.hooksJSON,
            permissions: 0o600
        )
    }

    private struct Locations {
        let home: URL
        let hooksDirectory: URL
        let hooksJSON: URL
        let script: URL
        let configTOML: URL?
    }

    private static func locations(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) throws -> Locations {
        let raw = environment["CODEX_HOME"]?.trimmingCharacters(in: .whitespacesAndNewlines)
        let home: URL
        if let raw, !raw.isEmpty {
            let candidate = URL(fileURLWithPath: raw).standardizedFileURL
            var isDirectory: ObjCBool = false
            guard fileManager.fileExists(atPath: candidate.path, isDirectory: &isDirectory), isDirectory.boolValue else {
                throw CodexNotificationHooksError.codexHomeMissing(candidate.path)
            }
            home = candidate.resolvingSymlinksInPath()
        } else {
            home = fileManager.homeDirectoryForCurrentUser
                .appendingPathComponent(".codex", isDirectory: true)
        }
        let hooksDirectory = home.appendingPathComponent("hooks", isDirectory: true)
        return Locations(
            home: home,
            hooksDirectory: hooksDirectory,
            hooksJSON: home.appendingPathComponent("hooks.json"),
            script: hooksDirectory.appendingPathComponent(CodexNotifyScript.fileName),
            configTOML: home.appendingPathComponent("config.toml")
        )
    }

    private static func loadRootIfPresent(at url: URL) throws -> [String: Any] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [:] }
        return try loadRoot(at: url)
    }

    private static func loadRoot(at url: URL) throws -> [String: Any] {
        do {
            return try CodexHookEditing.loadRoot(data: Data(contentsOf: url))
        } catch let error as CodexHookEditingError {
            throw CodexNotificationHooksError.invalidHooks(error)
        }
    }

    private static func enabledArguments(in root: [String: Any]) -> Set<String> {
        Set(CodexHookEditing.notificationEvents.filter { event in
            (try? CodexHookEditing.containsCommand(matching: marker, event: event, in: root)) == true
        })
    }

    private static func removeKnownLegacyNotify(at url: URL?) throws {
        guard let url, FileManager.default.fileExists(atPath: url.path) else { return }
        let data = try Data(contentsOf: url)
        guard let text = String(data: data, encoding: .utf8) else {
            throw CodexNotificationHooksError.invalidLegacyConfig
        }
        let result = CodexLegacyNotifyEditing.removeKnownNotify(from: text)
        switch result.status {
        case .absent:
            return
        case .conflict:
            throw CodexNotificationHooksError.legacyConflict
        case .removed:
            let permissions = (try? FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? NSNumber)
                .map { Int($0.uint16Value) } ?? 0o600
            try writeAtomically(Data(result.text.utf8), to: url, permissions: permissions)
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
