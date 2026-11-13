import Foundation

public struct StatisticsDateRange: Equatable, Sendable {
    public var startDay: String
    public var endDay: String

    public init(startDay: String, endDay: String) {
        self.startDay = startDay
        self.endDay = endDay
    }

    public static func dayString(_ date: Date, calendar: Calendar = .autoupdatingCurrent) -> String {
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        return String(format: "%04d-%02d-%02d", parts.year ?? 0, parts.month ?? 0, parts.day ?? 0)
    }

    public func bounds(now: Date = Date(), calendar: Calendar = .autoupdatingCurrent) -> (from: Double, to: Double)? {
        func parse(_ day: String) -> Date? {
            let pieces = day.split(separator: "-", omittingEmptySubsequences: false)
            guard pieces.count == 3, day.count == 10,
                  let year = Int(pieces[0]), let month = Int(pieces[1]), let date = Int(pieces[2]),
                  let value = calendar.date(from: DateComponents(year: year, month: month, day: date)),
                  Self.dayString(value, calendar: calendar) == day else { return nil }
            return calendar.startOfDay(for: value)
        }
        guard let start = parse(startDay), let end = parse(endDay), start <= end,
              end <= calendar.startOfDay(for: now),
              let nextDay = calendar.date(byAdding: .day, value: 1, to: end) else { return nil }
        return (start.timeIntervalSinceReferenceDate,
                min(now.timeIntervalSinceReferenceDate, nextDay.timeIntervalSinceReferenceDate.nextDown))
    }

    public func includesToday(now: Date = Date(), calendar: Calendar = .autoupdatingCurrent) -> Bool {
        endDay == Self.dayString(now, calendar: calendar)
    }
}
