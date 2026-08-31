import Foundation

/// Pure window-lifecycle state used by the menu-bar app.  Keeping the
/// transition independent from AppKit makes the Cmd+Tab contract testable
/// without opening a real window.
public enum AppWindowLifecycleEvent: Sendable {
    case opened
    case closed
    case hidden
}

public enum AppWindowLifecycleState: String, Equatable, Sendable {
    case accessory
    case regular

    public func applying(_ event: AppWindowLifecycleEvent) -> Self {
        switch event {
        case .opened:
            return .regular
        case .closed, .hidden:
            return .accessory
        }
    }
}
