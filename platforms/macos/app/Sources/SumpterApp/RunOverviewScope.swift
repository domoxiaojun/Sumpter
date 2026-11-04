import Foundation
import SumpterCore

enum RunOverviewScope {
    static let range = "today"

    static func dayStart(now: Date = Date(), calendar: Calendar = .autoupdatingCurrent) -> Double {
        calendar.startOfDay(for: now).timeIntervalSinceReferenceDate
    }

    static func isCurrent(_ snapshot: AdminWire.RuntimeFilter?, now: Date = Date(), calendar: Calendar = .autoupdatingCurrent) -> Bool {
        snapshot?.from == dayStart(now: now, calendar: calendar)
    }

    static func filter(
        kind: String? = nil,
        outcome: String? = nil,
        snapshot: AdminWire.RuntimeFilter? = nil,
        now: Date = Date(),
        calendar: Calendar = .autoupdatingCurrent
    ) -> AdminWire.RuntimeFilter {
        let current = isCurrent(snapshot, now: now, calendar: calendar) ? snapshot : nil
        return AdminWire.RuntimeFilter(
            kind: kind,
            outcome: outcome,
            from: dayStart(now: now, calendar: calendar),
            to: current?.to
        )
    }

    static func contains(_ event: RuntimeEvent, filter: AdminWire.RuntimeFilter) -> Bool {
        contains(timestamp: event.timestamp.timeIntervalSinceReferenceDate, filter: filter)
    }

    static func contains(timestamp: Double, filter: AdminWire.RuntimeFilter) -> Bool {
        return (filter.from.map { timestamp >= $0 } ?? true)
            && (filter.to.map { timestamp <= $0 } ?? true)
    }
}
