import AppKit
import Foundation
import ServiceManagement
import SwiftUI
import UserNotifications
import SumpterCore

@MainActor
extension AppModel {
    func addProviderAccount(
        idText: String,
        name: String,
        baseURLText: String,
        protocolName: String,
        enabled: Bool,
        apiKey: String,
        priority: Int = 0,
        stickyGroup: String = "",
        keepAlive: Bool = true
    ) async throws {
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        // 空 Key 合法:= 无鉴权转发(不发鉴权头,本地/内网 llama.cpp 等上游用),
        // 与引擎行为不变量一致;空 Key 仍允许无鉴权的本地/内网 HTTP 上游。
        let cleanKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty else {
            throw AppModelError.invalidInput("入口名称不能为空")
        }
        let url = try validatedURLString(baseURLText, field: "API 地址")
        guard let baseURL = URL(string: url) else {
            throw AppModelError.invalidInput("API 地址无效")
        }
        let trimmedID = idText.trimmingCharacters(in: .whitespacesAndNewlines)
        let id = try validatedProviderID(trimmedID.isEmpty ? uniqueEndpointID(cleanName) : trimmedID)
        let group = stickyGroup.trimmingCharacters(in: .whitespacesAndNewlines)
        guard priority >= 0 else {
            throw AppModelError.invalidInput("优先级必须是非负整数")
        }

        try await mutateConfig { config in
            guard config.endpoint(id: id) == nil else {
                throw AppModelError.invalidInput("入口 ID 已存在: \(id)")
            }
            config.endpoints.append(Endpoint(
                id: id,
                name: cleanName,
                baseURL: baseURL,
                protocolMode: try Self.endpointProtocolMode(protocolName),
                enabled: enabled,
                apiKey: cleanKey,
                priority: priority,
                stickyGroup: group.isEmpty ? nil : group,
                keepAlive: keepAlive
            ))
        }
    }

    func updateProviderAccount(
        id: String,
        name: String,
        baseURLText: String,
        protocolName: String,
        enabled: Bool,
        apiKey: String,
        priority: Int = 0,
        stickyGroup: String = "",
        keepAlive: Bool = false
    ) async throws {
        let cleanName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        // 空 Key 合法(无鉴权转发),同 addProviderAccount。
        let cleanKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanName.isEmpty else {
            throw AppModelError.invalidInput("入口名称不能为空")
        }
        let url = try validatedURLString(baseURLText, field: "API 地址")
        guard let baseURL = URL(string: url) else {
            throw AppModelError.invalidInput("API 地址无效")
        }
        let group = stickyGroup.trimmingCharacters(in: .whitespacesAndNewlines)
        guard priority >= 0 else {
            throw AppModelError.invalidInput("优先级必须是非负整数")
        }

        try await mutateConfig { config in
            let location = try Self.locate(endpointID: id, in: config)
            config.endpoints[location.endpoint].name = cleanName
            config.endpoints[location.endpoint].baseURL = baseURL
            config.endpoints[location.endpoint].apiKey = cleanKey
            config.endpoints[location.endpoint].protocolMode =
                try Self.endpointProtocolMode(protocolName)
            config.endpoints[location.endpoint].enabled = enabled
            config.endpoints[location.endpoint].priority = priority
            config.endpoints[location.endpoint].stickyGroup = group.isEmpty ? nil : group
            config.endpoints[location.endpoint].keepAlive = keepAlive
        }
    }

    /// 快捷启停只改一个字段，并在落盘/reload 任一步失败时恢复原状态。
    /// 回滚基于失败时的最新配置草稿，避免覆盖同时完成的其它字段修改。
    func setProviderEnabled(id: String, enabled: Bool) async throws {
        let location = try Self.locate(endpointID: id, in: config)
        let previousEnabled = config.endpoints[location.endpoint].enabled
        guard previousEnabled != enabled else { return }

        var draft = config
        draft.endpoints[location.endpoint].enabled = enabled
        draft.pruneDanglingEndpointReferences()
        try draft.validateModelGroups()
        config = draft

        do {
            try await persistConfigAndRefresh()
        } catch {
            let saveError = error
            var rollback = config
            if let rollbackLocation = try? Self.locate(endpointID: id, in: rollback),
               rollback.endpoints[rollbackLocation.endpoint].enabled == enabled {
                rollback.endpoints[rollbackLocation.endpoint].enabled = previousEnabled
                config = rollback
                do {
                    try await persistConfigAndRefresh()
                } catch {
                    throw AppModelError.invalidInput(
                        "入口启停保存失败（\(saveError.localizedDescription)），回滚也失败：\(error.localizedDescription)"
                    )
                }
            }
            throw saveError
        }
    }

    func deleteProviderAccounts(ids: Set<String>) async throws {
        guard !ids.isEmpty else { return }
        try await mutateConfig { config in
            config.endpoints.removeAll { ids.contains($0.id) }
        }
    }

    func moveProviderAccount(id: String, direction: Int) async throws {
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: id, in: config)
            let target = location.endpoint + direction
            guard config.endpoints.indices.contains(target) else {
                return
            }
            let endpoint = config.endpoints.remove(at: location.endpoint)
            config.endpoints.insert(endpoint, at: target)
        }
    }

    /// 多选整块移动(一次落盘;顺序语义 = TableSelection.moved,抵边行原地不动)。
    func moveProviderAccounts(ids: Set<String>, direction: Int) async throws {
        guard !ids.isEmpty else { return }
        try await mutateConfig { config in
            let order = config.endpoints.map(\.id)
            let target = TableSelection.moved(ids: order, selection: ids, direction: direction)
            guard target != order else { return }
            let byID = Dictionary(uniqueKeysWithValues: config.endpoints.map { ($0.id, $0) })
            config.endpoints = target.compactMap { byID[$0] }
        }
    }

    /// 拖拽入口后的任意位置移动。目标下标按移除源入口后的数组计算，
    /// 与 macOS Table 的拖放回调一致；仍走统一配置落盘/sidecar 刷新管线。
    func reorderProviderAccount(id: String, toIndex: Int) async throws {
        try await mutateConfig { config in
            let order = config.endpoints.map(\.id)
            let target = TableSelection.moved(id: id, orderedIDs: order, toIndex: toIndex)
            guard target != order else { return }
            let byID = Dictionary(uniqueKeysWithValues: config.endpoints.map { ($0.id, $0) })
            config.endpoints = target.compactMap { byID[$0] }
        }
    }

    // MARK: - 入口的模型映射

    func addProviderMapping(
        endpointID: String,
        clientPattern: String,
        upstreamModel: String,
        thinking: ThinkingMode,
        context: ContextMode,
        failoverTimeoutSeconds: Double? = nil,
        effort: ReasoningEffort? = nil
    ) async throws {
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            let client = ModelName.clean(clientPattern)
            let upstream = ModelName.clean(upstreamModel)
            guard !client.isEmpty else {
                throw AppModelError.invalidInput("客户端模型不能为空")
            }
            guard !config.endpoints[location.endpoint].hasMapping(clientPattern: client) else {
                throw AppModelError.invalidInput("该入口已存在客户端模型映射: \(client)")
            }
            config.endpoints[location.endpoint].mappings.append(ModelMapping(
                clientPattern: ModelPattern(client),
                upstreamModel: upstream,
                thinking: thinking,
                context: context,
                failoverTimeoutSeconds: failoverTimeoutSeconds,
                effort: effort
            ))
        }
    }

    /// 从已知模型目录里批量把选中的真实模型加成映射。
    func addProviderMappingsFromCatalog(endpointID: String, models: Set<String>) async throws {
        let specs: [(client: String, upstream: String)] = models.compactMap { raw in
            let original = ModelName.clean(raw)
            guard !original.isEmpty else { return nil }
            return (client: original, upstream: original)
        }
        guard !specs.isEmpty else { return }
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            var existing = Set(
                config.endpoints[location.endpoint].mappings.map {
                    ModelName.clean($0.clientPattern.rawValue)
                }
            )
            for spec in specs where !existing.contains(spec.client) {
                config.endpoints[location.endpoint].mappings.append(ModelMapping(
                    clientPattern: ModelPattern(spec.client),
                    upstreamModel: spec.upstream,
                    thinking: .passthrough,
                    context: ModelName.defaultContext(for: spec.client)
                ))
                existing.insert(spec.client)
            }
        }
    }

    /// 某入口已获取但尚未映射的真实模型(供「从已知模型添加」挑选)。
    func unmappedCatalogModels(endpointID: String) -> [String] {
        guard let endpoint = config.endpoint(id: endpointID) else { return [] }
        let mapped = Set(endpoint.mappings.map { ModelName.clean($0.clientPattern.rawValue) })
        var seen: Set<String> = []
        var result: [String] = []
        for model in endpoint.catalog.models {
            let cleaned = ModelName.clean(model)
            guard !cleaned.isEmpty, !mapped.contains(cleaned), !seen.contains(cleaned) else { continue }
            seen.insert(cleaned)
            result.append(cleaned)
        }
        return result
    }

    /// `effort` 传 nil = 自动跟随客户端；编辑器把「自适应 + 留空」清洗为 nil 落盘。
    func updateProviderMapping(
        endpointID: String,
        mappingID: String,
        clientPattern: String,
        upstreamModel: String,
        thinking: ThinkingMode,
        context: ContextMode,
        failoverTimeoutSeconds: Double?,
        effort: ReasoningEffort? = nil
    ) async throws {
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            let endpoint = config.endpoints[location.endpoint]
            guard let mappingIndex = endpoint.mappings.firstIndex(where: { $0.id == mappingID }) else {
                throw AppModelError.invalidInput("模型映射不存在")
            }
            let client = ModelName.clean(clientPattern)
            let upstream = ModelName.clean(upstreamModel)
            guard !client.isEmpty else {
                throw AppModelError.invalidInput("客户端模型不能为空")
            }
            guard !endpoint.hasMapping(clientPattern: client, excluding: mappingID) else {
                throw AppModelError.invalidInput("该入口已存在客户端模型映射: \(client)")
            }
            // Capability declarations are part of the routing contract. The
            // editor currently does not expose a picker, so an update must
            // carry the existing values forward instead of silently turning
            // an explicit video/live/files mapping back into name inference.
            let existingCapabilities = endpoint.mappings[mappingIndex].capabilities
            config.endpoints[location.endpoint].mappings[mappingIndex] = ModelMapping(
                clientPattern: ModelPattern(client),
                upstreamModel: upstream,
                thinking: thinking,
                context: context,
                failoverTimeoutSeconds: failoverTimeoutSeconds,
                capabilities: existingCapabilities,
                effort: effort
            )
        }
    }

    func deleteProviderMapping(endpointID: String, mappingID: String) async throws {
        try await deleteProviderMappings(endpointID: endpointID, mappingIDs: [mappingID])
    }

    func deleteProviderMappings(endpointID: String, mappingIDs: Set<String>) async throws {
        guard !mappingIDs.isEmpty else { return }
        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            config.endpoints[location.endpoint].mappings.removeAll {
                mappingIDs.contains($0.id)
            }
        }
    }

    func fetchProviderModels(endpointID: String) async throws {
        fetchingModelEndpointIDs.insert(endpointID)
        defer { fetchingModelEndpointIDs.remove(endpointID) }

        guard config.endpoint(id: endpointID) != nil else {
            throw AppModelError.invalidInput("入口不存在: \(endpointID)")
        }
        guard let admin else {
            throw AppModelError.invalidInput("代理服务尚未启动，无法获取模型")
        }
        let result = try await admin.providerModels(endpointID: endpointID)
        let models = ModelCatalog.deduplicatedModels(result.models)
        let source = result.source
        let stamp = modelCatalogTimestamp(result.updatedAt)

        try await mutateConfig { config in
            let location = try Self.locate(endpointID: endpointID, in: config)
            config.endpoints[location.endpoint].catalog = ModelCatalog(
                models: models,
                source: source,
                status: "已获取",
                error: "",
                updatedAt: stamp
            )
        }
        flash("已获取 \(models.count) 个模型")
    }

    func markFetchModelsFailure(endpointID: String, error: Error) async {
        let message = String((error.localizedDescription.isEmpty ? "\(error)" : error.localizedDescription).prefix(300))
        do {
            try await mutateConfig { config in
                let location = try Self.locate(endpointID: endpointID, in: config)
                config.endpoints[location.endpoint].catalog.status = "获取失败"
                config.endpoints[location.endpoint].catalog.error = message
                config.endpoints[location.endpoint].catalog.updatedAt = modelCatalogTimestamp()
            }
        } catch {
            lastError = "\(error)"
        }
    }

    // MARK: - 分流规则

    func updateFeatureRuleAndSave(
        id: String,
        enabled: Bool,
        model: String,
        effortOverride: ReasoningEffort?,
        protocolOverride: ProviderProtocol?,
        endpointID: String?,
        toolTypePrefix: String,
        systemContains: String,
        messagesContain: String,
        modelEquals: String
    ) async throws {
        let cleanedModel = ModelName.clean(model)
        guard !cleanedModel.isEmpty else {
            throw AppModelError.invalidInput("目标模型不能为空")
        }
        let trimmedEndpointID = endpointID?.trimmingCharacters(in: .whitespacesAndNewlines)
        let targetEndpointID = (trimmedEndpointID?.isEmpty == false) ? trimmedEndpointID : nil

        try await mutateConfig { config in
            if let targetEndpointID {
                guard config.endpoint(id: targetEndpointID) != nil else {
                    throw AppModelError.invalidInput("Provider 不存在: \(targetEndpointID)")
                }
            }
            guard let index = config.featureRules.firstIndex(where: { $0.id == id }) else {
                throw AppModelError.invalidInput("分流规则不存在: \(id)")
            }
            config.featureRules[index].enabled = enabled
            config.featureRules[index].target = RouteTarget(
                model: cleanedModel,
                protocolOverride: protocolOverride,
                endpointID: targetEndpointID,
                effortOverride: effortOverride
            )
            if let canonical = BuiltInFeatureRules.canonicalRule(id: id) {
                config.featureRules[index].name = canonical.name
                config.featureRules[index].match = canonical.match
            } else {
                config.featureRules[index].match = FeatureMatch(
                    toolTypePrefix: nilIfBlank(toolTypePrefix),
                    systemContains: nilIfBlank(systemContains),
                    messagesContain: nilIfBlank(messagesContain),
                    modelEquals: nilIfBlank(modelEquals)
                )
            }
        }
    }

    func updateFeatureRule(
        id: String,
        enabled: Bool,
        model: String,
        effortOverride: ReasoningEffort? = nil,
        protocolOverride: ProviderProtocol? = nil,
        endpointID: String? = nil,
        toolTypePrefix: String,
        systemContains: String,
        messagesContain: String,
        modelEquals: String
    ) {
        Task {
            do {
                try await updateFeatureRuleAndSave(
                    id: id,
                    enabled: enabled,
                    model: model,
                    effortOverride: effortOverride,
                    protocolOverride: protocolOverride,
                    endpointID: endpointID,
                    toolTypePrefix: toolTypePrefix,
                    systemContains: systemContains,
                    messagesContain: messagesContain,
                    modelEquals: modelEquals
                )
            } catch {
                lastError = "\(error)"
                flash("更新分流规则失败")
            }
        }
    }

    // MARK: - 转发与重试参数(全局)

    func updateRetryPolicyAndSave(
        responseTimeoutText: String,
        streamIdleTimeoutText: String,
        max500RetriesText: String,
        failoverOn500: Bool = true,
        retryDelaySecondsText: String,
        passThroughRetryDelay: Bool = true,
        maxDeferredRoundsText: String,
        maxRetryDurationSecondsText: String,
        sessionStickyRetriesText: String
    ) async throws {
        let tuning = try mapInputValidation {
            try InputValidation.retryPolicy(
                responseTimeoutText: responseTimeoutText,
                streamIdleTimeoutText: streamIdleTimeoutText,
                max500RetriesText: max500RetriesText,
                failoverOn500: failoverOn500,
                retryDelaySecondsText: retryDelaySecondsText,
                passThroughRetryDelay: passThroughRetryDelay,
                maxDeferredRoundsText: maxDeferredRoundsText,
                maxRetryDurationSecondsText: maxRetryDurationSecondsText,
                sessionStickyRetriesText: sessionStickyRetriesText
            )
        }
        try await mutateConfig { config in
            config.retry = RetryPolicy(
                responseTimeoutSeconds: tuning.responseTimeoutSeconds,
                streamIdleTimeoutSeconds: tuning.streamIdleTimeoutSeconds,
                max500Retries: tuning.max500Retries,
                failoverOn500: tuning.failoverOn500,
                retryDelaySeconds: tuning.retryDelaySeconds,
                passThroughRetryDelay: tuning.passThroughRetryDelay,
                maxDeferredRounds: tuning.maxDeferredRounds,
                maxRetryDurationSeconds: tuning.maxRetryDurationSeconds,
                sessionStickyRetries: tuning.sessionStickyRetries
            )
        }
    }

    // MARK: - Claude Code 配置备份

    func backupClaudeSettings() {
        do {
            try ClaudeNotificationHooks.backupSettings()
            flash("Claude Code 配置已备份")
        } catch {
            lastError = "\(error)"
            flash("备份 Claude Code 配置失败")
        }
    }

    func restoreClaudeSettingsBackup() {
        do {
            try ClaudeNotificationHooks.restoreBackup()
            refreshNotificationHookState()
            flash("Claude Code 配置已还原")
        } catch {
            lastError = "\(error)"
            flash("还原 Claude Code 配置失败")
        }
    }

    // MARK: - 入站认证

    func setInboundAuthToken(_ token: String) {
        Task {
            do {
                try await mutateConfig { config in
                    config.listener.authToken = token.trimmingCharacters(in: .whitespacesAndNewlines)
                }
            } catch {
                lastError = "\(error)"
                flash("更新入站认证失败")
            }
        }
    }

    func clearInboundAuthToken() {
        setInboundAuthToken("")
    }

    /// 提交监听配置(安全页草稿的唯一落地入口)。
    /// 走 mutateConfig 统一管线:归一化 → 落盘 → 推给引擎(必要时重启进程)。
    /// 端口变更时同步重写通知 hook 脚本(脚本内嵌端口,不重写会静默失联)。
    func updateListener(host: String, port: Int, allowedCIDRs: [String]) {
        Task {
            do {
                let portChanged = port != config.listener.port
                try await mutateConfig { draft in
                    draft.listener.host = host
                    draft.listener.port = port
                    draft.listener.allowedCIDRs = allowedCIDRs
                }
                if portChanged {
                    do {
                        try ClaudeNotificationHooks.rewriteScriptIfEnabled(port: port)
                        try CodexNotificationHooks.rewriteScriptIfEnabled(port: port)
                        try GrokNotificationHooks.rewriteScriptIfEnabled(port: port)
                        refreshNotificationHookState()
                    } catch {
                        flash("通知脚本更新失败,请重新切换一次通知开关")
                    }
                }
            } catch {
                lastError = "\(error)"
                flash("保存监听配置失败")
            }
        }
    }

    // MARK: - 配置改写基础设施

    /// 所有配置修改的唯一入口:在草稿上改,归一化内建规则,落盘并刷新引擎。
    func mutateConfig(_ body: (inout AppConfig) throws -> Void) async throws {
        let previous = config
        var draft = previous
        try body(&draft)
        draft.pruneDanglingEndpointReferences()
        try draft.validateModelGroups()
        draft.normalizeBuiltInFeatureRules()
        config = draft
        do {
            try await persistConfigAndRefresh()
        } catch {
            let saveError = error
            // Do not overwrite a newer edit that arrived while persistence
            // was suspended. Otherwise restore both the visible and disk draft.
            if config == draft {
                config = previous
                do {
                    try await persistConfigAndRefresh()
                } catch {
                    throw AppModelError.invalidInput("保存失败（\(saveError.localizedDescription)），回滚也失败：\(error.localizedDescription)")
                }
            }
            throw saveError
        }
    }

    struct EndpointLocation {
        let endpoint: Int
    }

    static func locate(endpointID: String, in config: AppConfig) throws -> EndpointLocation {
        guard let endpoint = config.endpoints.firstIndex(where: { $0.id == endpointID }) else {
            throw AppModelError.invalidInput("Provider 不存在: \(endpointID)")
        }
        return EndpointLocation(endpoint: endpoint)
    }

    static func endpointProtocolMode(_ rawValue: String) throws -> EndpointProtocolMode {
        guard let mode = EndpointProtocolMode(rawValue: rawValue) else {
            throw AppModelError.invalidInput("入口协议无效: \(rawValue)")
        }
        return mode
    }

    func presentMigrationNoticeIfNeeded(_ notice: ConfigMigrationNotice?) {
        guard let notice else { return }
        let defaultsKey = "sumpter.configMigrationNotice.\(notice.id)"
        guard !UserDefaults.standard.bool(forKey: defaultsKey) else { return }
        UserDefaults.standard.set(true, forKey: defaultsKey)
        configMigrationNotice = notice
        let expanded = notice.autoEndpointIDs.count
        let suffix = expanded > 0 ? "，其中 \(expanded) 个入口已转为自动（三协议）" : ""
        flash("配置已迁移到 v\(notice.toSchema)\(suffix)；旧文件已备份")
    }

    func validatedURLString(_ text: String, field: String) throws -> String {
        try mapInputValidation { try InputValidation.url(text, field: field) }
    }

    func validatedProviderID(_ text: String) throws -> String {
        try mapInputValidation { try InputValidation.providerID(text) }
    }

    /// 把 Core 的 `InputValidationError` 归一成 AppModel 的 `invalidInput`，保持既有对外错误契约。
    func mapInputValidation<T>(_ body: () throws -> T) throws -> T {
        do {
            return try body()
        } catch let error as InputValidationError {
            throw AppModelError.invalidInput(error.message)
        }
    }


}
