import Foundation
import SumpterCore

/// 菜单栏与运行页共用的总状态指示。
///
/// **进程生命周期优先于上游健康**:sidecar 架构下「引擎进程本身没了」是全新的故障态,
/// 老的单进程 actor 架构不存在它;若只显示上游健康,进程崩了会被误显示为「已停止」,
/// 用户无从区分「我自己停的」和「它自己死的」。
public enum StatusIndicator: Equatable, Sendable {
    case stopped        // 已停止(用户主动)
    case starting       // 启动中:spawn + 等握手
    case crashed        // 引擎异常退出(非主动停止)
    case unreachable    // 进程在但 admin 不通(假死)
    case health(ProxyHealth)

    public var dotStyle: DotStyle {
        switch self {
        case .stopped: .quiet
        case .starting: .starting
        case .crashed: .fault
        case .unreachable: .fault
        case .health(let health):
            switch health {
            case .healthy: .ok
            case .idle: .neutral
            case .degraded: .warn
            case .down: .fault
            case .stopped: .quiet
            }
        }
    }

    public var label: String {
        switch self {
        case .stopped: "已停止"
        case .starting: "启动中"
        case .crashed: "引擎异常退出"
        case .unreachable: "引擎无响应"
        case .health(let health):
            switch health {
            case .healthy: "正常"
            case .idle: "空闲"
            case .degraded: "降级"
            case .down: "故障"
            case .stopped: "已停止"
            }
        }
    }

    /// 需要用户注意(运行页/菜单栏加醒目提示)。
    public var needsAttention: Bool {
        switch self {
        case .crashed, .unreachable: true
        case .health(let health): health == .down
        default: false
        }
    }

    public enum DotStyle: Equatable, Sendable {
        case ok, neutral, warn, fault, quiet, starting
    }
}

/// sidecar 进程生命周期(AppModel 持有;与上游健康正交)。
public enum SidecarState: Equatable, Sendable {
    case stopped
    case starting
    case running
    case unreachable
    case crashed(Int32)

    public var isActive: Bool {
        self == .starting || self == .running || self == .unreachable
    }
}
