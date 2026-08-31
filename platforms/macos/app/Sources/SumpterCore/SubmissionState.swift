import Foundation

/// 设置页 Sheet 异步提交的状态机（纯逻辑，可单测）。
///
/// 统一各编辑 Sheet 的提交流程语义：
/// - 提交进行中禁止重复提交（`begin()` 返回 false 表示已在提交，调用方直接 return）；
/// - 开始提交即清空上一次错误；
/// - 成功 → 回到 idle（由调用方关闭 Sheet）；失败 → 保留错误文案且回到非提交态，Sheet 不关闭。
public enum SubmissionState: Equatable, Sendable {
    case idle
    case submitting
    case failed(String)

    /// 是否正在提交（用于按钮 loading / disabled）。
    public var isSubmitting: Bool {
        self == .submitting
    }

    /// 当前错误文案；无错误时为空串（供 `SheetErrorText` 直接使用）。
    public var errorText: String {
        if case .failed(let message) = self {
            return message
        }
        return ""
    }

    /// 尝试进入提交态。已在提交中返回 false（调用方应直接 return），否则清错误并返回 true。
    public mutating func begin() -> Bool {
        guard self != .submitting else {
            return false
        }
        self = .submitting
        return true
    }

    /// 提交成功：回到 idle。
    public mutating func succeed() {
        self = .idle
    }

    /// 提交失败：记录错误文案并回到非提交态。
    public mutating func fail(_ message: String) {
        self = .failed(message)
    }
}
