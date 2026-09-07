import Foundation

/// UI-only grouping of models explicitly added to endpoint mappings.
/// Fetched catalogs are discovery data, not configured selection candidates.
public struct ModelGroupCatalogCategory: Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let models: [String]

    public var identity: String { id }

    public init(id: String, title: String, models: [String]) {
        self.id = id
        self.title = title
        self.models = models
    }
}

public enum ModelGroupCatalog {
    public static let otherID = "__other__"

    public static func categories(endpoints: [Endpoint]) -> [ModelGroupCatalogCategory] {
        categories(models: endpoints.flatMap { endpoint in
            endpoint.mappings.map { $0.clientPattern.rawValue }
        })
    }

    public static func categories(models: [String]) -> [ModelGroupCatalogCategory] {
        var normalized: [String: String] = [:]
        for raw in models {
            let model = ModelName.clean(raw).trimmingCharacters(in: .whitespacesAndNewlines)
            let key = model.lowercased()
            guard !key.isEmpty else { continue }
            normalized[key] = normalized[key] ?? model
        }
        var grouped: [String: [String]] = [:]
        for model in normalized.values {
            let id = prefix(for: model)
            grouped[id, default: []].append(model)
        }
        return grouped
            .map { id, values in
                ModelGroupCatalogCategory(
                    id: id,
                    title: id == otherID ? "其他" : id,
                    models: values.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
                )
            }
            .sorted {
                if $0.id == otherID { return false }
                if $1.id == otherID { return true }
                return $0.title.localizedCaseInsensitiveCompare($1.title) == .orderedAscending
            }
    }

    public static func prefix(for model: String) -> String {
        let cleaned = ModelName.clean(model).trimmingCharacters(in: .whitespacesAndNewlines)
        let separator = cleaned.firstIndex(where: { $0 == "-" || $0 == "_" || $0 == ":" || $0 == "/" })
        let token = String(cleaned[..<(separator ?? cleaned.endIndex)]).lowercased()
        let family = token.replacingOccurrences(of: #"\d+(?:\.\d+)*$"#, with: "", options: .regularExpression)
        guard !family.isEmpty, separator != nil || family != token else { return otherID }
        return family
    }

    /// Intersect the group's scope with this endpoint's added mappings. Respect
    /// configured wildcards without expanding fetched catalogs or local aliases.
    /// Existing selections are not mutated by this read-only projection.
    public static func availableModels(
        endpoint: Endpoint?, groupModels: [String]
    ) -> [String] {
        guard let endpoint else { return [] }
        let supported = endpoint.mappings.map { ModelName.clean($0.clientPattern.rawValue) }
        let candidates = groupModels + supported
        return Array(Set(candidates.map(ModelName.clean))).filter { model in
            guard !model.isEmpty, groupModels.contains(where: { ModelName.matches(model, pattern: $0) }) else { return false }
            return supported.contains { ModelName.matches(model, pattern: $0) }
        }.sorted()
    }
}
