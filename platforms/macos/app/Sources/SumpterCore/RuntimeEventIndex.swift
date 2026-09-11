public extension RuntimeEvent {
    /// 历史页、详情和实时缓存会短暂包含同一 ID，不能假设拼接后的键唯一。
    /// 已结束事件优先于进行中事件；同阶段选较新的时间戳，相同时间保留首项。
    /// 若胜出的记录是列表投影，继续保留另一份记录中已加载的详情。
    static func indexedByID(_ events: [RuntimeEvent]) -> [String: RuntimeEvent] {
        Dictionary(events.map { ($0.id, $0) }, uniquingKeysWith: { current, incoming in
            let preferIncoming = current.isInFlight != incoming.isInFlight
                ? current.isInFlight
                : incoming.timestamp > current.timestamp
            return preferIncoming
                ? current.mergingProjection(incoming)
                : incoming.mergingProjection(current)
        })
    }

    /// 历史分页和 SSE 不会同时刷新；已在历史页中的事件不能再次出现在实时区。
    static func liveOverlay(
        _ events: [RuntimeEvent],
        excluding persistedIDs: Set<String>,
        filter: RuntimeEventKindFilter
    ) -> [RuntimeEvent] {
        let indexed = indexedByID(events)
        var seen = Set<String>()
        return events.compactMap { item in
            guard seen.insert(item.id).inserted,
                  !persistedIDs.contains(item.id),
                  let event = indexed[item.id], event.isInFlight,
                  filter == .all || event.kind == filter.rawValue else { return nil }
            return event
        }
    }
}
