import Foundation

public struct ResponsesWebSocketConnections: Codable, Equatable, Sendable {
    public let total: Int
    public let awaitingFirstMessage: Int
    public let guardianAwaitingFirstMessage: Int
    public let connectingUpstream: Int
    public let relaying: Int
    public let oldestFirstMessageWaitMS: Int?
}

/// A failed refresh preserves the last observation; absence from a successful
/// response means an older daemon, not zero connections.
public struct ResponsesConnectionsObservation: Equatable, Sendable {
    public private(set) var value: ResponsesWebSocketConnections?
    public private(set) var loaded = false
    public private(set) var error: String?

    public init() {}

    public mutating func receive(_ value: ResponsesWebSocketConnections?) {
        self.value = value
        loaded = true
        error = nil
    }

    public mutating func fail(_ error: String) {
        self.error = error
    }

    public var emptyMessage: String? {
        guard value == nil else { return nil }
        if error != nil { return "连接数据暂不可用" }
        return loaded ? "当前版本未提供" : "正在读取连接数据…"
    }

    public var errorMessage: String? {
        guard error != nil else { return nil }
        return value == nil ? "更新失败" : "更新失败，保留上次数据"
    }

    public func displayed(autoRefresh: Bool, frozen: Self?) -> Self {
        autoRefresh ? self : (frozen ?? self)
    }
}
