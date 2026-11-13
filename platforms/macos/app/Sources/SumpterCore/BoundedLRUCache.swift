import Foundation

/// Cost is response bytes, not an estimate of the allocator's resident memory.
public struct BoundedLRUCache<Key: Hashable, Value> {
    private var values: [Key: (value: Value, cost: Int)] = [:]
    private var order: [Key] = []
    public private(set) var cost = 0
    public let countLimit: Int
    public let costLimit: Int
    public var count: Int { values.count }

    public init(countLimit: Int = 200, costLimit: Int = 8 * 1024 * 1024) {
        self.countLimit = max(0, countLimit)
        self.costLimit = max(0, costLimit)
    }

    public func peek(_ key: Key) -> Value? { values[key]?.value }

    public mutating func value(for key: Key) -> Value? {
        guard let value = values[key]?.value else { return nil }
        order.removeAll { $0 == key }
        order.append(key)
        return value
    }

    public mutating func insert(_ value: Value, for key: Key, cost newCost: Int) {
        remove(key)
        let newCost = max(0, newCost)
        guard countLimit > 0, newCost <= costLimit else { return }
        while !order.isEmpty && (values.count >= countLimit || cost > costLimit - newCost) {
            remove(order[0])
        }
        values[key] = (value, newCost)
        order.append(key)
        cost += newCost
    }

    public mutating func remove(_ key: Key) {
        if let old = values.removeValue(forKey: key) { cost -= old.cost }
        order.removeAll { $0 == key }
    }

    public mutating func removeAll() {
        values.removeAll()
        order.removeAll()
        cost = 0
    }
}
