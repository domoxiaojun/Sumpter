import XCTest
@testable import SumpterCore

final class AccountSchedulerTests: XCTestCase {
    func testStickyPreferredGoesFirst() {
        // 上次成功的账号被同一会话粘住,排最前(即便 home 指向别处)。
        let out = AccountScheduler.ordered(
            groupIDs: ["acc1", "acc2", "acc3"],
            health: [:],
            stickyPreferred: "acc3",
            homeIndex: 0,
            now: Date()
        )
        XCTAssertEqual(out.first, "acc3")
        XCTAssertEqual(Set(out), ["acc1", "acc2", "acc3"])
    }

    func testCoolingAccountSinksToBottomEvenIfHome() {
        // home 本指向 acc2,但 acc2 正冷却 → 沉底,别的账号先试。
        let now = Date()
        let health = ["acc2": AccountScheduler.Health(coolingUntil: now.addingTimeInterval(60))]
        let out = AccountScheduler.ordered(
            groupIDs: ["acc1", "acc2", "acc3"],
            health: health,
            stickyPreferred: nil,
            homeIndex: 1,
            now: now
        )
        XCTAssertEqual(out.last, "acc2")
        XCTAssertNotEqual(out.first, "acc2")
    }

    func testStickyButCoolingFallsBack() {
        // 粘的账号正冷却 → 冷却压倒粘性,沉底,不强粘坏账号。
        let now = Date()
        let health = ["acc3": AccountScheduler.Health(coolingUntil: now.addingTimeInterval(60))]
        let out = AccountScheduler.ordered(
            groupIDs: ["acc1", "acc2", "acc3"],
            health: health,
            stickyPreferred: "acc3",
            homeIndex: 0,
            now: now
        )
        XCTAssertEqual(out.last, "acc3")
    }

    func testExpiredCoolingTreatedHealthy() {
        // 冷却已过期 → 视为健康,回到 home 旋转顺序。
        let now = Date()
        let health = ["acc2": AccountScheduler.Health(coolingUntil: now.addingTimeInterval(-1))]
        let out = AccountScheduler.ordered(
            groupIDs: ["acc1", "acc2"],
            health: health,
            stickyPreferred: nil,
            homeIndex: 1,
            now: now
        )
        XCTAssertEqual(out, ["acc2", "acc1"])
    }

    func testSingleAccountUnchanged() {
        let out = AccountScheduler.ordered(
            groupIDs: ["acc1"],
            health: ["acc1": AccountScheduler.Health(coolingUntil: Date().addingTimeInterval(60))],
            stickyPreferred: "acc1",
            homeIndex: 0,
            now: Date()
        )
        XCTAssertEqual(out, ["acc1"])
    }
}
