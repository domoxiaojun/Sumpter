import Foundation

public struct ClientBinding: Codable, Equatable, Sendable {
    public var client: String = ""
    public var variant: String?
    public var relaySource: String?
    public init() {}
}
public struct ClientIdentitySettings: Codable, Equatable, Sendable {
    public var tokenBindings: [String: ClientBinding] = [:]
    public var reportingAliases: [String: String] = [:]
    public var uaAliases: [String: String] = [:]
    public var relayEndpoints: [String: String] = [:]
    public init() {}
    enum CodingKeys: String, CodingKey { case tokenBindings, reportingAliases, uaAliases, relayEndpoints }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        tokenBindings = try c.decodeIfPresent([String:ClientBinding].self, forKey: .tokenBindings) ?? [:]
        reportingAliases = try c.decodeIfPresent([String:String].self, forKey: .reportingAliases) ?? [:]
        uaAliases = try c.decodeIfPresent([String:String].self, forKey: .uaAliases) ?? [:]
        relayEndpoints = try c.decodeIfPresent([String:String].self, forKey: .relayEndpoints) ?? [:]
    }
    public static func validProduct(_ value: String) -> Bool {
        (1...64).contains(value.utf8.count) && value.utf8.allSatisfy { (97...122).contains($0) || (48...57).contains($0) || [46,45,95].contains($0) }
    }
    public static func parseAliases(_ text: String, catalog: Bool = true) throws -> [String:String] {
        var values: [String:String] = [:]
        for line in text.split(separator: "\n").map({ $0.trimmingCharacters(in: .whitespaces) }).filter({ !$0.isEmpty }) {
            let parts = line.split(separator: "=", omittingEmptySubsequences: false).map(String.init)
            guard parts.count == 2 else { throw invalid("每行填写一个等号分隔的映射") }
            let validKey = catalog ? validProduct(parts[0]) : (!parts[0].isEmpty && parts[0].utf8.count <= 128)
            let validValue = catalog ? ClientRegistry.names[parts[1]] != nil : validProduct(parts[1])
            guard validKey, validValue, values[parts[0]] == nil else {
                throw invalid("每行填写 产品标识=客户端ID，标识不能重复")
            }
            values[parts[0]] = parts[1]
        }
        guard values.count <= 128 else { throw invalid("映射最多 128 条") }
        return values
    }
    public static func aliasesText(_ values: [String:String]) -> String {
        values.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: "\n")
    }
    public func validate() throws {
        guard tokenBindings.count <= 129, reportingAliases.count <= 128, uaAliases.count <= 128, relayEndpoints.count <= 128 else { throw Self.invalid("客户端身份规则超过上限") }
        for (id, binding) in tokenBindings {
            guard !id.isEmpty, id.utf8.count <= 128, id.utf8.allSatisfy({ (65...90).contains($0) || (97...122).contains($0) || (48...57).contains($0) || [45,95].contains($0) }), Self.validProduct(binding.client), binding.relaySource.map(Self.validProduct) ?? true,
                  binding.variant.map({ ClientRegistry.variants.contains($0) }) ?? true else { throw Self.invalid("客户端绑定无效") }
        }
        for (key,value) in Array(reportingAliases) + Array(uaAliases) {
            guard Self.validProduct(key), ClientRegistry.names[value] != nil else { throw Self.invalid("客户端映射无效") }
        }
        for (id,source) in relayEndpoints { guard !id.isEmpty, id.utf8.count <= 128, Self.validProduct(source) else { throw Self.invalid("中继入口配置无效") } }
    }
    private static func invalid(_ message:String) -> NSError { NSError(domain: "Sumpter.ClientIdentity", code:1, userInfo:[NSLocalizedDescriptionKey:message]) }
}
