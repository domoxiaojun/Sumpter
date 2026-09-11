import Foundation

public enum ModelGroupSchedulingStrategy: String, Codable, Sendable, CaseIterable {
    case priority
    case randomSticky
    case roundRobinSticky

    public var displayName: String {
        switch self {
        case .priority: "优先级顺序"
        case .randomSticky: "同优先级随机并保持会话粘性"
        case .roundRobinSticky: "同优先级按顺序轮询并保持会话粘性"
        }
    }
}

public struct ModelGroup: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var enabled: Bool
    public var priority: Int
    public var schedulingStrategy: ModelGroupSchedulingStrategy
    public var models: [String]
    public var bindings: [ModelGroupBinding]
    public init(id: String = UUID().uuidString, name: String = "新模型组", enabled: Bool = true,
                priority: Int = 0, schedulingStrategy: ModelGroupSchedulingStrategy = .priority,
                models: [String] = [], bindings: [ModelGroupBinding] = []) {
        self.id = id; self.name = name; self.enabled = enabled; self.priority = priority
        self.schedulingStrategy = schedulingStrategy; self.models = models; self.bindings = bindings
    }
    enum CodingKeys: String, CodingKey { case id, name, enabled, priority, schedulingStrategy, models, bindings }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? id
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        priority = try c.decodeIfPresent(Int.self, forKey: .priority) ?? 0
        schedulingStrategy = try c.decodeIfPresent(ModelGroupSchedulingStrategy.self, forKey: .schedulingStrategy) ?? .priority
        models = try c.decodeIfPresent([String].self, forKey: .models) ?? []
        bindings = try c.decodeIfPresent([ModelGroupBinding].self, forKey: .bindings) ?? []
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id); try c.encode(name, forKey: .name)
        try c.encode(enabled, forKey: .enabled); try c.encode(priority, forKey: .priority)
        if schedulingStrategy != .priority { try c.encode(schedulingStrategy, forKey: .schedulingStrategy) }
        try c.encode(models, forKey: .models); try c.encode(bindings, forKey: .bindings)
    }
}

public struct ModelGroupBinding: Codable, Equatable, Sendable, Identifiable {
    public var id: String { endpointID }
    public var endpointID: String
    public var enabled: Bool
    public var priority: Int
    /// nil = all group models supported by the endpoint; [] = none.
    public var models: [String]?
    public var overrides: [ModelGroupModelOverride]
    public init(endpointID: String, enabled: Bool = true, priority: Int = 0, models: [String]? = [],
                overrides: [ModelGroupModelOverride] = []) {
        self.endpointID = endpointID; self.enabled = enabled; self.priority = priority
        self.models = models; self.overrides = overrides
    }
    enum CodingKeys: String, CodingKey { case endpointID, enabled, priority, models, overrides }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        endpointID = try c.decode(String.self, forKey: .endpointID)
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        priority = try c.decodeIfPresent(Int.self, forKey: .priority) ?? 0
        models = try c.decodeIfPresent([String].self, forKey: .models)
        overrides = try c.decodeIfPresent([ModelGroupModelOverride].self, forKey: .overrides) ?? []
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(endpointID, forKey: .endpointID); try c.encode(enabled, forKey: .enabled)
        try c.encode(priority, forKey: .priority); try c.encode(models, forKey: .models)
        if !overrides.isEmpty { try c.encode(overrides, forKey: .overrides) }
    }
}

public struct ModelGroupModelOverride: Codable, Equatable, Sendable, Identifiable {
    public var id: String { model }
    public var model: String
    public var upstreamModel: String?
    public var priority: Int?
    public init(model: String, upstreamModel: String? = nil, priority: Int? = nil) {
        self.model = model; self.upstreamModel = upstreamModel; self.priority = priority
    }
}

public extension AppConfig {
    mutating func addEndpointToLibrary(_ endpoint: Endpoint) {
        // Freeze legacy membership before insertion. New entries require an
        // explicit binding even when the next save migrates a legacy config.
        migrateModelGroups()
        endpoints.append(endpoint)
    }

    var hasRoutableModel: Bool {
        guard modelGroups != nil else {
            return endpoints.contains { $0.enabled && !$0.mappings.isEmpty }
        }
        return routingEndpoints(for: "").contains { !$0.mappings.isEmpty }
    }

    mutating func migrateModelGroups() {
        guard modelGroups == nil else { return }
        var models: [String] = []
        let bindings = endpoints.map { e in
            var selected: [String] = []
            for mapping in e.mappings {
                let name = ModelName.clean(mapping.clientPattern.rawValue)
                guard !name.isEmpty else { continue }
                if !models.contains(name) { models.append(name) }
                if !selected.contains(name) { selected.append(name) }
            }
            return ModelGroupBinding(endpointID: e.id, priority: e.priority, models: selected)
        }
        modelGroups = endpoints.isEmpty ? [] : [ModelGroup(id: "default", name: "默认模型组", models: models, bindings: bindings)]
    }

    mutating func pruneDanglingEndpointReferences() {
        let ids = Set(endpoints.map(\.id))
        if var groups = modelGroups {
            for i in groups.indices {
                groups[i].bindings.removeAll { !ids.contains($0.endpointID) }
                let models = groups[i].models
                for j in groups[i].bindings.indices {
                    if let selected = groups[i].bindings[j].models {
                        groups[i].bindings[j].models = selected.filter { m in models.contains { ModelName.matches(m, pattern: $0) } }
                    }
                    let selected = groups[i].bindings[j].models
                    groups[i].bindings[j].overrides.removeAll { o in
                        !models.contains { ModelName.matches(o.model, pattern: $0) }
                        || (selected.map { list in !list.contains { ModelName.matches(o.model, pattern: $0) } } ?? false)
                    }
                }
            }
            modelGroups = groups
        }
        for i in featureRules.indices {
            if let id = featureRules[i].target.endpointID, !ids.contains(id) { featureRules[i].target.endpointID = nil }
        }
    }

    /// 默认组(id="default")的绑定顺序与优先级跟随入口库:按 endpoints 数组顺序
    /// 重排、binding.priority = endpoint.priority、剔除悬空引用;不自动补新增
    /// 入口(保留刻意排除)。与 Rust `normalized()` 的 `sync_default_group_bindings`
    /// 是同一契约,在 mutateConfig 落盘前调用,保证两端写出的配置一致。
    mutating func syncDefaultGroupBindings() {
        guard var groups = modelGroups, let index = groups.firstIndex(where: { $0.id == "default" }) else { return }
        let byEndpoint = Dictionary(groups[index].bindings.map { ($0.endpointID, $0) }, uniquingKeysWith: { first, _ in first })
        groups[index].bindings = endpoints.compactMap { endpoint in
            guard var binding = byEndpoint[endpoint.id] else { return nil }
            binding.priority = max(0, endpoint.priority)
            return binding
        }
        modelGroups = groups
    }

    func validateModelGroups() throws {
        var ids = Set<String>()
        for g in modelGroups ?? [] {
            guard !g.id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, ids.insert(g.id).inserted,
                  g.priority >= 0 else { throw ConfigStoreError.invalidField("模型组 ID 或优先级无效") }
            var models = Set<String>()
            for m in g.models {
                let m = ModelName.clean(m)
                guard !m.isEmpty, models.insert(m).inserted,
                      !m.contains("*") || (m.hasSuffix("*") && m.filter { $0 == "*" }.count == 1)
                else { throw ConfigStoreError.invalidField("模型组含空、重复或无效模型模式") }
            }
            var members = Set<String>()
            for b in g.bindings {
                guard endpoint(id: b.endpointID) != nil, members.insert(b.endpointID).inserted, b.priority >= 0
                else { throw ConfigStoreError.invalidField("模型组入口引用或优先级无效") }
                if let selected = b.models {
                    guard selected.allSatisfy({
                            let clean = ModelName.clean($0)
                            return !clean.isEmpty
                                && (!clean.contains("*") || (clean.hasSuffix("*") && clean.filter { $0 == "*" }.count == 1))
                        }),
                        Set(selected.map(ModelName.clean)).count == selected.count,
                          selected.allSatisfy({ m in g.models.contains { ModelName.matches(m, pattern: $0) } })
                    else { throw ConfigStoreError.invalidField("入口选择了组外或重复模型") }
                }
                var seen = Set<String>()
                for o in b.overrides {
                    let clean = ModelName.clean(o.model)
                    guard !clean.isEmpty, !clean.contains("*"), seen.insert(clean).inserted,
                          g.models.contains(where: { ModelName.matches(o.model, pattern: $0) }),
                          b.models.map({ list in list.contains { ModelName.matches(o.model, pattern: $0) } }) ?? true,
                          o.priority.map({ $0 >= 0 }) ?? true
                    else { throw ConfigStoreError.invalidField("模型覆盖无效") }
                }
            }
        }
    }

    /// App-side planning/preview mirrors core's group projection. Credentials
    /// remain on the original endpoint; the projected copy is never persisted.
    func routingEndpoints(for model: String) -> [Endpoint] {
        guard let groups = modelGroups else { return endpoints }
        func intersect(_ a: String, _ b: String) -> String? {
            if ModelName.matches(b, pattern: a) { return b }
            if ModelName.matches(a, pattern: b) { return a }
            return nil
        }
        func specificity(_ pattern: String) -> (Int, Int) {
            let cleaned = ModelName.clean(pattern)
            if !cleaned.isEmpty, !cleaned.hasSuffix("*") { return (2, cleaned.utf8.count) }
            return (1, (cleaned.hasSuffix("*") ? String(cleaned.dropLast()) : cleaned).utf8.count)
        }
        var output: [Endpoint] = []
        let ordered = groups.enumerated().filter { $0.element.enabled }.sorted {
            $0.element.priority == $1.element.priority ? $0.offset < $1.offset : $0.element.priority < $1.element.priority
        }
        for (rank, pair) in ordered.enumerated() {
            let g = pair.element
            for b in g.bindings where b.enabled {
                guard let source = endpoint(id: b.endpointID), source.enabled else { continue }
                let patterns = b.models.map { selected in g.models.flatMap { g in selected.compactMap { intersect(g, $0) } } } ?? g.models
                var e = source
                e.modelGroupID = g.id; e.modelGroupRank = rank
                e.modelGroupSchedulingStrategy = g.schedulingStrategy
                e.priority = b.overrides.first { ModelName.clean($0.model) == ModelName.clean(model) }?.priority ?? b.priority
                if g.id != "default" { e.stickyGroup = "model-group:\(g.id.utf8.count):\(g.id):\(source.stickyGroup ?? source.id)" }
                e.mappings = []
                var inherited: [(ModelMapping, (Int, Int))] = []
                for original in source.mappings {
                    for p in patterns {
                        if let pattern = intersect(original.clientPattern.rawValue, p) {
                            var m = original; m.clientPattern = ModelPattern(pattern)
                            let rank = specificity(original.clientPattern.rawValue)
                            if let index = inherited.firstIndex(where: { $0.0 == m }) {
                                if rank <= inherited[index].1 { continue }
                                inherited.remove(at: index)
                            }
                            let index = inherited.firstIndex {
                                $0.0.clientPattern == m.clientPattern && rank > $0.1
                            } ?? inherited.count
                            inherited.insert((m, rank), at: index)
                        }
                    }
                }
                e.mappings = inherited.map(\.0)
                // Groups only narrow endpoint support, including stale selections.
                for o in b.overrides {
                    if let upstream = o.upstreamModel {
                        var exact = e.mappings.filter { ModelName.matches(o.model, pattern: $0.clientPattern.rawValue) }
                        exact.sort { specificity($0.clientPattern.rawValue) > specificity($1.clientPattern.rawValue) }
                        for i in exact.indices { exact[i].clientPattern = ModelPattern(o.model); exact[i].upstreamModel = upstream }
                        e.mappings = exact + e.mappings
                    }
                }
                output.append(e)
            }
        }
        return output
    }
}

public extension ModelName {
    static func matches(_ model: String, pattern: String) -> Bool { ModelPattern(pattern).matches(model) }
}
