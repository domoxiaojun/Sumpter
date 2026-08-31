import Foundation

/// 特征分流规则「目标模型」候选项的派生逻辑。
///
/// 候选 = 扁平 Provider 候选序列中各入口显式映射声明的客户端模型；钉住具体入口时只给那个入口能接的模型。
public enum FeatureRouteCandidates {
    /// 扁平 Provider 候选序列承接的客户端模型，去重后保持出现顺序。
    public static func models(for endpoints: [Endpoint]) -> [String] {
        var seen: Set<String> = []
        return endpoints.flatMap(\.mappings).map(\.clientPattern.rawValue).filter { raw in
            let cleaned = ModelName.clean(raw)
            guard !cleaned.isEmpty, seen.insert(cleaned).inserted else { return false }
            return true
        }
    }

    /// 某个入口能接的客户端模型：显式映射优先，其次是「获取模型」拉回来的目录。
    public static func models(for endpoint: Endpoint) -> [String] {
        var seen: Set<String> = []
        var output: [String] = []
        let declared = endpoint.mappings.map(\.clientPattern.rawValue)
        for raw in declared + endpoint.catalog.models {
            let cleaned = ModelName.clean(raw)
            guard !cleaned.isEmpty, !seen.contains(cleaned) else {
                continue
            }
            seen.insert(cleaned)
            output.append(raw)
        }
        return output
    }

    /// 按分流规则目标给出候选：钉了入口就只给该入口的，否则给全部 Provider。
    public static func models(config: AppConfig, endpointID: String?) -> [String] {
        if let endpointID, let endpoint = config.endpoint(id: endpointID) {
            return models(for: endpoint)
        }
        return models(for: config.endpoints)
    }

    /// Deprecated source shim for extensions compiled against the old pool API.
    @available(*, deprecated, message: "Provider 池已移除；请使用 endpoints")
    public static func models(for pool: Pool) -> [String] {
        models(for: pool.endpoints)
    }

    @available(*, deprecated, message: "Provider 池已移除；请使用 models(for: Endpoint)")
    public static func models(for endpoint: Endpoint, in pool: Pool) -> [String] {
        models(for: endpoint)
    }

    @available(*, deprecated, message: "Provider 池已移除；请使用 models(config:endpointID:)")
    public static func models(config: AppConfig, poolID _: String, endpointID: String?) -> [String] {
        models(config: config, endpointID: endpointID)
    }
}
