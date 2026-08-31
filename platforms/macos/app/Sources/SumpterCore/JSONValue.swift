import Foundation

public enum JSONValue: Codable, Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value):
            try container.encode(value)
        case .number(let value):
            try container.encode(value)
        case .bool(let value):
            try container.encode(value)
        case .object(let value):
            try container.encode(value)
        case .array(let value):
            try container.encode(value)
        case .null:
            try container.encodeNil()
        }
    }

    public var stringValue: String? {
        if case .string(let value) = self {
            return value
        }
        return nil
    }

    public var intValue: Int? {
        if case .number(let value) = self {
            return Int(value)
        }
        return nil
    }

    public var doubleValue: Double? {
        if case .number(let value) = self {
            return value
        }
        return nil
    }

    public var boolValue: Bool? {
        if case .bool(let value) = self {
            return value
        }
        return nil
    }

    public subscript(key: String) -> JSONValue? {
        if case .object(let value) = self {
            return value[key]
        }
        return nil
    }

    public func canonicalJSONString() -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        guard let data = try? encoder.encode(self) else {
            return "null"
        }
        return String(data: data, encoding: .utf8) ?? "null"
    }

    public func pythonStyleJSONString() -> String {
        switch self {
        case .string(let value):
            return quoted(value)
        case .number(let value):
            if value.rounded() == value {
                return "\(Int(value))"
            }
            return "\(value)"
        case .bool(let value):
            return value ? "true" : "false"
        case .null:
            return "null"
        case .array(let values):
            return "[" + values.map { $0.pythonStyleJSONString() }.joined(separator: ", ") + "]"
        case .object(let object):
            let preferred = ["type", "text", "content", "role", "id", "name", "input", "tool_use_id"]
            let preferredKeys = preferred.filter { object[$0] != nil }
            let rest = object.keys.filter { !preferredKeys.contains($0) }.sorted()
            return (preferredKeys + rest)
                .map { key in
                    "\(quoted(key)): \(object[key]?.pythonStyleJSONString() ?? "null")"
                }
                .joined(separator: ", ")
                .wrapped(prefix: "{", suffix: "}")
        }
    }

    private func quoted(_ value: String) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: [value], options: [.withoutEscapingSlashes])) ?? Data("[\"\"]".utf8)
        let encoded = String(data: data, encoding: .utf8) ?? "[\"\"]"
        return String(encoded.dropFirst().dropLast())
    }
}

private extension String {
    func wrapped(prefix: String, suffix: String) -> String {
        prefix + self + suffix
    }
}
