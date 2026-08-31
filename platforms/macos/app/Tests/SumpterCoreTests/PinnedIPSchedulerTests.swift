import XCTest
@testable import SumpterCore

/// 覆盖 pinned IP 健康感知排序:冷却靠后、近期成功靠前、滚动分散、DNS 兜底。
final class PinnedIPSchedulerTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_000_000)

    func testCoolingDownIPsGoLast() {
        let candidates: [String?] = ["1.1.1.1", "2.2.2.2", "3.3.3.3"]
        let health: [String: PinnedIPScheduler.Health] = [
            "2.2.2.2": .init(coolingUntil: t0.addingTimeInterval(60))  // 冷却中
        ]
        let ordered = PinnedIPScheduler.ordered(candidates: candidates, health: health, now: t0, rotation: 0)
        XCTAssertEqual(ordered.last as? String, "2.2.2.2", "冷却中的 IP 应排最后")
        XCTAssertFalse((ordered.prefix(2).compactMap { $0 }).contains("2.2.2.2"))
    }

    func testExpiredCooldownIsAvailableAgain() {
        let candidates: [String?] = ["1.1.1.1", "2.2.2.2"]
        let health: [String: PinnedIPScheduler.Health] = [
            "2.2.2.2": .init(coolingUntil: t0.addingTimeInterval(-1))  // 冷却已过期
        ]
        let ordered = PinnedIPScheduler.ordered(candidates: candidates, health: health, now: t0, rotation: 0)
        XCTAssertEqual(Set(ordered.compactMap { $0 }), ["1.1.1.1", "2.2.2.2"], "过期冷却应重新可用")
    }

    func testRecentlySucceededTriedFirstNewestWins() {
        let candidates: [String?] = ["1.1.1.1", "2.2.2.2", "3.3.3.3"]
        let health: [String: PinnedIPScheduler.Health] = [
            "1.1.1.1": .init(lastSuccess: t0.addingTimeInterval(-100)),
            "3.3.3.3": .init(lastSuccess: t0.addingTimeInterval(-10))   // 更近的成功
        ]
        let ordered = PinnedIPScheduler.ordered(candidates: candidates, health: health, now: t0, rotation: 0)
        XCTAssertEqual(ordered.first as? String, "3.3.3.3", "最近成功的先试")
        XCTAssertEqual(ordered[1] as? String, "1.1.1.1", "较早成功的其次")
        XCTAssertEqual(ordered.last as? String, "2.2.2.2", "未试过的在成功过的之后")
    }

    func testRotationSpreadsUntried() {
        let candidates: [String?] = ["a", "b", "c", "d"]
        let r0 = PinnedIPScheduler.ordered(candidates: candidates, health: [:], now: t0, rotation: 0)
        let r1 = PinnedIPScheduler.ordered(candidates: candidates, health: [:], now: t0, rotation: 1)
        XCTAssertEqual(r0.first as? String, "a")
        XCTAssertEqual(r1.first as? String, "b", "滚动偏移应分散首选,避免每次都撞同一个")
    }

    func testDNSFallbackTreatedAsAvailable() {
        let candidates: [String?] = ["1.1.1.1", nil]
        let health: [String: PinnedIPScheduler.Health] = [
            "1.1.1.1": .init(coolingUntil: t0.addingTimeInterval(60))
        ]
        let ordered = PinnedIPScheduler.ordered(candidates: candidates, health: health, now: t0, rotation: 0)
        XCTAssertNil(ordered.first ?? "x", "冷却中的 IP 排在 DNS 兜底之后,首位应是 nil(DNS)")
    }

    func testSingleCandidateUnchanged() {
        XCTAssertEqual(PinnedIPScheduler.ordered(candidates: ["only"], health: [:], now: t0, rotation: 5).count, 1)
        XCTAssertEqual(PinnedIPScheduler.ordered(candidates: [nil], health: [:], now: t0, rotation: 5).count, 1)
    }
}
