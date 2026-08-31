import Darwin
import Foundation
import Security

public enum SumpterPaths {
    public static let bundleIdentifier = "org.kkl.sumpter"

    public static func appSupportDirectory(
        fileManager: FileManager = .default
    ) throws -> URL {
        let base = try fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let directory = base.appendingPathComponent("Sumpter", isDirectory: true)
        if !fileManager.fileExists(atPath: directory.path) {
            try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        return directory
    }

    public static func configURL(fileManager: FileManager = .default) throws -> URL {
        try appSupportDirectory(fileManager: fileManager).appendingPathComponent("config.json")
    }

    public static func logURL(fileManager: FileManager = .default) throws -> URL {
        try appSupportDirectory(fileManager: fileManager).appendingPathComponent("proxy.log")
    }

    public static func statsURL(fileManager: FileManager = .default) throws -> URL {
        try appSupportDirectory(fileManager: fileManager).appendingPathComponent("stats.json")
    }

    public static func controlTokenURL(fileManager: FileManager = .default) throws -> URL {
        try appSupportDirectory(fileManager: fileManager).appendingPathComponent(".control_token")
    }

    public static func autostartURL(fileManager: FileManager = .default) throws -> URL {
        try appSupportDirectory(fileManager: fileManager).appendingPathComponent(".autostart")
    }
}

public struct ConfigStore: Sendable {
    public var url: URL

    public init(url: URL) {
        self.url = url
    }

    public func load() throws -> AppConfig {
        try loadWithMigration().config
    }

    /// 读取当前 schema v6；旧 v3/v4/v5 文件只在首次读取时迁移一次。
    /// 迁移前先验证原始结构，成功写回后再复读校验；任何一步失败都会恢复原字节。
    public func loadWithMigration() throws -> ConfigLoadResult {
        let original = try Data(contentsOf: url)
        guard var root = try JSONSerialization.jsonObject(with: original) as? [String: Any],
              let schemaVersion = root["schemaVersion"] as? Int else {
            throw ConfigStoreError.unsupportedSchema(nil)
        }

        if schemaVersion == AppConfig.currentSchemaVersion {
            let config = try decodeCurrentV6(data: original, root: root)
            return ConfigLoadResult(config: config)
        }
        guard [3, 4, 5].contains(schemaVersion) else {
            throw ConfigStoreError.unsupportedSchema(schemaVersion)
        }

        let migration = try migrateLegacyRoot(&root, schemaVersion: schemaVersion)
        root["schemaVersion"] = AppConfig.currentSchemaVersion
        let candidate = try JSONSerialization.data(
            withJSONObject: root,
            options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        )
        let migrated = try decodeCurrentV6(data: candidate, root: root)
        let normalized = try encodeCurrentV6(migrated)
        guard let normalizedRoot = try JSONSerialization.jsonObject(with: normalized) as? [String: Any] else {
            throw ConfigStoreError.invalidRoot
        }
        _ = try decodeCurrentV6(data: normalized, root: normalizedRoot)

        let backup = try backupCurrent(suffix: "schema-v6")
        do {
            try writeAtomically(normalized)
            let verifiedData = try Data(contentsOf: url)
            guard let verifiedRoot = try JSONSerialization.jsonObject(with: verifiedData) as? [String: Any] else {
                throw ConfigStoreError.invalidRoot
            }
            let verified = try decodeCurrentV6(data: verifiedData, root: verifiedRoot)
            let backupName = backup?.lastPathComponent ?? ""
            return ConfigLoadResult(
                config: verified,
                migrationNotice: ConfigMigrationNotice(
                    id: "schema-v\(schemaVersion)-to-v6-\(backupName)",
                    fromSchema: schemaVersion,
                    toSchema: AppConfig.currentSchemaVersion,
                    backupFile: backupName,
                    endpointCount: migration.endpointCount,
                    expandedLegacyPassthroughEndpoints: migration.autoEndpointIDs.count,
                    autoEndpointIDs: migration.autoEndpointIDs,
                    removedFields: migration.removedFields
                )
            )
        } catch {
            do {
                try writeAtomically(original)
            } catch let rollbackError {
                throw ConfigStoreError.rollbackFailed(
                    migration: String(describing: error),
                    rollback: String(describing: rollbackError)
                )
            }
            throw error
        }
    }

    private func decodeCurrentV6(data: Data, root: [String: Any]) throws -> AppConfig {
        guard root["schemaVersion"] as? Int == AppConfig.currentSchemaVersion else {
            throw ConfigStoreError.unsupportedSchema(root["schemaVersion"] as? Int)
        }
        if root["pools"] != nil {
            throw ConfigStoreError.legacyField("pools")
        }
        if let listener = root["listener"] as? [String: Any],
           listener["inboundDialectPassthrough"] != nil {
            throw ConfigStoreError.legacyField("listener.inboundDialectPassthrough")
        }
        try validateEndpointProtocols(in: root)
        if let rules = root["featureRules"] as? [[String: Any]] {
            for (index, rule) in rules.enumerated() {
                if let target = rule["target"] as? [String: Any],
                   target["poolID"] != nil || target["poolId"] != nil {
                    throw ConfigStoreError.legacyField("featureRules[\(index)].target.poolID")
                }
            }
        }
        return try JSONDecoder().decode(AppConfig.self, from: data).normalizedBuiltInFeatureRules()
    }

    private struct LegacyMigrationMetadata {
        var endpointCount = 0
        var autoEndpointIDs: [String] = []
        var removedFields: [String] = []
    }

    /// 将 v3/v4/v5 的池容器一次性展平为 v6 `endpoints`。此处是唯一允许读取
    /// 旧池字段的地方；迁移完成后强类型模型和保存路径都只接受扁平形状。
    private func migrateLegacyRoot(
        _ root: inout [String: Any],
        schemaVersion: Int
    ) throws -> LegacyMigrationMetadata {
        var metadata = LegacyMigrationMetadata()

        var listener = root["listener"] as? [String: Any] ?? [:]
        guard root["listener"] == nil || root["listener"] is [String: Any] else {
            throw ConfigStoreError.invalidField("listener")
        }
        var passthrough = false
        if let raw = listener.removeValue(forKey: "inboundDialectPassthrough") {
            guard let value = raw as? Bool else {
                throw ConfigStoreError.invalidField("listener.inboundDialectPassthrough")
            }
            passthrough = value
            metadata.removedFields.append("listener.inboundDialectPassthrough")
        }
        root["listener"] = listener

        var flattened: [[String: Any]] = []
        if let rawPools = root.removeValue(forKey: "pools") {
            guard var pools = rawPools as? [[String: Any]] else {
                throw ConfigStoreError.invalidField("pools")
            }
            // 早期版本把 primary 放在首位；其他池保持文件顺序追加。
            if let primaryIndex = pools.firstIndex(where: {
                ($0["role"] as? String) == "primary" || ($0["id"] as? String) == "primary"
            }), primaryIndex != 0 {
                let primary = pools.remove(at: primaryIndex)
                pools.insert(primary, at: 0)
            }

            var usedIDs = Set<String>()
            var nextPriority = 0
            for (poolIndex, pool) in pools.enumerated() {
                guard let endpoints = pool["endpoints"] as? [[String: Any]] else {
                    throw ConfigStoreError.invalidField("pools[\(poolIndex)].endpoints")
                }

                // v5 可能仍含池级 globalModels。先复制到本池没有显式映射的入口，
                // 再把池壳移除，避免旧规则在迁移后静默丢失。
                var inheritedMappings: [[String: Any]] = []
                if let rawGlobal = pool["globalModels"] {
                    guard let rules = rawGlobal as? [[String: Any]] else {
                        throw ConfigStoreError.invalidField("pools[\(poolIndex)].globalModels")
                    }
                    inheritedMappings = try rules.enumerated().map { ruleIndex, rule in
                        guard let rawPattern = rule["pattern"] as? String else {
                            throw ConfigStoreError.invalidField(
                                "pools[\(poolIndex)].globalModels[\(ruleIndex)].pattern"
                            )
                        }
                        let pattern = rawPattern.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !pattern.isEmpty else {
                            throw ConfigStoreError.invalidField(
                                "pools[\(poolIndex)].globalModels[\(ruleIndex)].pattern"
                            )
                        }
                        return [
                            "clientPattern": pattern,
                            "context": rule["context"] ?? ContextMode.standard.rawValue,
                            "thinking": rule["thinking"] ?? ThinkingMode.adaptive.rawValue,
                            "upstreamModel": "",
                        ]
                    }
                    metadata.removedFields.append("pools[].globalModels")
                }

                for endpointIndex in endpoints.indices {
                    var endpoint = endpoints[endpointIndex]
                    guard let endpointID = endpoint["id"] as? String,
                          !endpointID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                        throw ConfigStoreError.invalidField(
                            "pools[\(poolIndex)].endpoints[\(endpointIndex)].id"
                        )
                    }
                    guard usedIDs.insert(endpointID).inserted else {
                        throw ConfigStoreError.invalidField("endpoints.id 重复: \(endpointID)")
                    }
                    if schemaVersion == 3 {
                        endpoint.removeValue(forKey: "searchDialect")
                        metadata.removedFields.append("pools[].endpoints[].searchDialect")
                        if passthrough {
                            endpoint["protocol"] = EndpointProtocolMode.auto.rawValue
                            metadata.autoEndpointIDs.append(endpointID)
                        } else if endpoint["protocol"] == nil {
                            endpoint["protocol"] = EndpointProtocolMode.anthropic.rawValue
                        }
                    } else {
                        try validateEndpoint(endpoint, field: "pools[\(poolIndex)].endpoints[\(endpointIndex)]")
                    }
                    if !inheritedMappings.isEmpty {
                        if let rawMappings = endpoint["mappings"] {
                            guard let explicit = rawMappings as? [[String: Any]] else {
                                throw ConfigStoreError.invalidField(
                                    "pools[\(poolIndex)].endpoints[\(endpointIndex)].mappings"
                                )
                            }
                            if explicit.isEmpty { endpoint["mappings"] = inheritedMappings }
                        } else {
                            endpoint["mappings"] = inheritedMappings
                        }
                    }
                    if poolIndex > 0 {
                        nextPriority = max(nextPriority + 1, 10)
                        endpoint["priority"] = nextPriority
                    } else if let priority = endpoint["priority"] as? Int {
                        nextPriority = max(nextPriority, priority)
                    }
                    flattened.append(endpoint)
                }
            }
            metadata.endpointCount = flattened.count
        } else {
            // A schema v5 file may already have been partially flattened. Keep
            // its order and only normalize the schema marker.
            guard let endpoints = root["endpoints"] as? [[String: Any]]
                ?? (root["endpoints"] == nil ? [] : nil) else {
                throw ConfigStoreError.invalidField("endpoints")
            }
            for (index, endpoint) in endpoints.enumerated() {
                try validateEndpoint(endpoint, field: "endpoints[\(index)]")
            }
            flattened = endpoints
            metadata.endpointCount = endpoints.count
        }

        root["endpoints"] = flattened
        let endpointIDs = Set(flattened.compactMap { $0["id"] as? String })
        if var rules = root["featureRules"] as? [[String: Any]] {
            for index in rules.indices {
                guard var target = rules[index]["target"] as? [String: Any] else { continue }
                if target.removeValue(forKey: "poolID") != nil || target.removeValue(forKey: "poolId") != nil {
                    metadata.removedFields.append("featureRules[].target.poolID")
                }
                if let endpointID = target["endpointID"] as? String,
                   !endpointID.isEmpty,
                   !endpointIDs.contains(endpointID) {
                    throw ConfigStoreError.invalidField("featureRules[\(index)].target.endpointID")
                }
                rules[index]["target"] = target
            }
            root["featureRules"] = rules
        }
        metadata.removedFields = Array(Set(metadata.removedFields)).sorted()
        return metadata
    }

    private func validateEndpoint(_ endpoint: [String: Any], field: String) throws {
        if endpoint["protocols"] != nil {
            throw ConfigStoreError.legacyField("\(field).protocols")
        }
        guard let rawProtocol = endpoint["protocol"] else {
            throw ConfigStoreError.missingField("\(field).protocol")
        }
        guard let protocolName = rawProtocol as? String,
              EndpointProtocolMode(rawValue: protocolName) != nil else {
            throw ConfigStoreError.invalidField("\(field).protocol")
        }
    }

    /// 当前 v6 入口协议仍需显式声明；Endpoint 解码器的默认值只用于新建对象。
    private func validateEndpointProtocols(in root: [String: Any]) throws {
        guard let endpoints = root["endpoints"] as? [[String: Any]] else {
            if root["endpoints"] == nil { return }
            throw ConfigStoreError.invalidField("endpoints")
        }
        for (index, endpoint) in endpoints.enumerated() {
            try validateEndpoint(endpoint, field: "endpoints[\(index)]")
        }
    }

    public func save(_ config: AppConfig) throws {
        try writeAtomically(try encodeCurrentV6(config))
    }

    private func encodeCurrentV6(_ config: AppConfig) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        var stored = config
        stored.schemaVersion = AppConfig.currentSchemaVersion
        return try encoder.encode(stored.normalizedBuiltInFeatureRules())
    }

    /// 同目录临时文件 + fsync + rename + 父目录 fsync，确保保存和迁移共用同一套落盘语义。
    private func writeAtomically(_ data: Data) throws {
        let fileManager = FileManager.default
        let directory = url.deletingLastPathComponent()
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        let tempURL = directory.appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        var renamed = false
        defer {
            if !renamed {
                try? fileManager.removeItem(at: tempURL)
            }
        }

        try data.write(to: tempURL, options: .withoutOverwriting)
        try fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: tempURL.path)
        try synchronizeFile(at: tempURL)
        try posixRename(from: tempURL, to: url)
        renamed = true
        try synchronizeDirectory(at: directory)
    }

    private func synchronizeFile(at fileURL: URL) throws {
        let handle = try FileHandle(forWritingTo: fileURL)
        defer { try? handle.close() }
        try handle.synchronize()
    }

    private func synchronizeDirectory(at directoryURL: URL) throws {
        let descriptor = Darwin.open(directoryURL.path, O_RDONLY)
        guard descriptor >= 0 else { throw currentPOSIXError() }
        defer { Darwin.close(descriptor) }
        guard Darwin.fsync(descriptor) == 0 else { throw currentPOSIXError() }
    }

    private func posixRename(from source: URL, to destination: URL) throws {
        let result = source.path.withCString { sourcePath in
            destination.path.withCString { destinationPath in
                Darwin.rename(sourcePath, destinationPath)
            }
        }
        guard result == 0 else { throw currentPOSIXError() }
    }

    private func currentPOSIXError() -> POSIXError {
        POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }

    /// 结构性变更前另存一份原文件,命名 `config.before-<suffix>-<yyyyMMdd-HHmmss-SSS>.json`。
    /// 文件不存在时返回 nil。
    @discardableResult
    public func backupCurrent(suffix: String) throws -> URL? {
        guard FileManager.default.fileExists(atPath: url.path) else {
            return nil
        }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyyMMdd-HHmmss-SSS"
        let stem = url.deletingPathExtension().lastPathComponent
        let directory = url.deletingLastPathComponent()
        let baseName = "\(stem).before-\(suffix)-\(formatter.string(from: Date()))"
        var backupURL = directory.appendingPathComponent("\(baseName).json")
        var suffixIndex = 1
        while FileManager.default.fileExists(atPath: backupURL.path) {
            backupURL = directory.appendingPathComponent("\(baseName)-\(suffixIndex).json")
            suffixIndex += 1
        }
        try FileManager.default.copyItem(at: url, to: backupURL)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: backupURL.path)
        try synchronizeFile(at: backupURL)
        try synchronizeDirectory(at: backupURL.deletingLastPathComponent())
        return backupURL
    }
}

public enum ConfigStoreError: Error, Equatable, Sendable, LocalizedError {
    case unsupportedSchema(Int?)
    case legacyField(String)
    case missingField(String)
    case invalidField(String)
    case invalidRoot
    case rollbackFailed(migration: String, rollback: String)

    public var errorDescription: String? {
        switch self {
        case .unsupportedSchema(let version):
            "不支持的配置 schema：\(version.map(String.init) ?? "缺失")（支持 v3/v4/v5 自动迁移或 v6）"
        case .legacyField(let field):
            "schema v6 不允许旧字段：\(field)"
        case .missingField(let field):
            "schema v6 缺少必填字段：\(field)"
        case .invalidField(let field):
            "配置字段类型或值无效：\(field)"
        case .invalidRoot:
            "迁移后的配置根节点不是 JSON 对象"
        case .rollbackFailed(let migration, let rollback):
            "配置迁移失败且恢复原文件失败：migration=\(migration)，rollback=\(rollback)"
        }
    }
}

public struct ConfigMigrationNotice: Codable, Equatable, Sendable {
    public var id: String
    public var fromSchema: Int
    public var toSchema: Int
    public var backupFile: String
    public var endpointCount: Int
    public var expandedLegacyPassthroughEndpoints: Int
    public var autoEndpointIDs: [String]
    public var removedFields: [String]

    public init(
        id: String,
        fromSchema: Int,
        toSchema: Int,
        backupFile: String,
        endpointCount: Int,
        expandedLegacyPassthroughEndpoints: Int? = nil,
        autoEndpointIDs: [String],
        removedFields: [String] = []
    ) {
        self.id = id
        self.fromSchema = fromSchema
        self.toSchema = toSchema
        self.backupFile = backupFile
        self.endpointCount = endpointCount
        self.expandedLegacyPassthroughEndpoints = expandedLegacyPassthroughEndpoints ?? autoEndpointIDs.count
        self.autoEndpointIDs = autoEndpointIDs
        self.removedFields = removedFields
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case fromSchema
        case toSchema
        case backupFile
        case endpointCount
        case expandedLegacyPassthroughEndpoints
        case autoEndpointIDs
        case convertedToAutoEndpointIds
        case removedFields
    }

    public init(from decoder: Decoder) throws {
        let keyed = try decoder.container(keyedBy: CodingKeys.self)
        let endpointIDs = try keyed.decodeIfPresent([String].self, forKey: .convertedToAutoEndpointIds)
            ?? keyed.decodeIfPresent([String].self, forKey: .autoEndpointIDs)
            ?? []
        self.init(
            id: try keyed.decode(String.self, forKey: .id),
            fromSchema: try keyed.decode(Int.self, forKey: .fromSchema),
            toSchema: try keyed.decode(Int.self, forKey: .toSchema),
            backupFile: try keyed.decode(String.self, forKey: .backupFile),
            endpointCount: try keyed.decode(Int.self, forKey: .endpointCount),
            expandedLegacyPassthroughEndpoints: try keyed.decodeIfPresent(
                Int.self,
                forKey: .expandedLegacyPassthroughEndpoints
            ),
            autoEndpointIDs: endpointIDs,
            removedFields: try keyed.decodeIfPresent([String].self, forKey: .removedFields) ?? []
        )
    }

    public func encode(to encoder: Encoder) throws {
        var keyed = encoder.container(keyedBy: CodingKeys.self)
        try keyed.encode(id, forKey: .id)
        try keyed.encode(fromSchema, forKey: .fromSchema)
        try keyed.encode(toSchema, forKey: .toSchema)
        try keyed.encode(backupFile, forKey: .backupFile)
        try keyed.encode(endpointCount, forKey: .endpointCount)
        try keyed.encode(expandedLegacyPassthroughEndpoints, forKey: .expandedLegacyPassthroughEndpoints)
        try keyed.encode(autoEndpointIDs, forKey: .autoEndpointIDs)
        try keyed.encode(removedFields, forKey: .removedFields)
    }
}

public struct ConfigLoadResult: Equatable, Sendable {
    public var config: AppConfig
    public var migrationNotice: ConfigMigrationNotice?

    public init(config: AppConfig, migrationNotice: ConfigMigrationNotice? = nil) {
        self.config = config
        self.migrationNotice = migrationNotice
    }
}

public enum ControlTokenStore {
    public static func ensureToken(at url: URL) throws -> String {
        if FileManager.default.fileExists(atPath: url.path),
           let value = try? String(contentsOf: url, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines),
           !value.isEmpty {
            return value
        }
        var bytes = [UInt8](repeating: 0, count: 16)
        let status = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        let token: String
        if status == errSecSuccess {
            token = bytes.map { String(format: "%02x", $0) }.joined()
        } else {
            token = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        }
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try token.write(to: url, atomically: true, encoding: .utf8)
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        return token
    }
}
