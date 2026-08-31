import AppKit
import Combine
import SumpterCore
import Sparkle
import SwiftUI

/// 菜单栏状态项:**左键直接打开主窗口,右键弹操作菜单**。
///
/// 为什么用 AppKit 的 `NSStatusItem` 而不是 SwiftUI `MenuBarExtra`:
/// `menuBarExtraStyle` 只能整体选 `.window` 或 `.menu`,左右键行为必然一致,
/// 做不到左右键分流。改用 NSStatusItem 后由 `handleClick` 按事件类型分派。
@MainActor
final class StatusItemController: NSObject {
    private let model: AppModel
    private let statusItem: NSStatusItem
    private let settingsWindow: SettingsWindowController
    /// 仅在打包时提供了 appcast 后启用，避免本地 SwiftPM 调试包启动时弹出
    /// Sparkle 配置错误提示。生产包由 package-app.sh 写入 SUFeedURL。
    private let updaterController: SPUStandardUpdaterController?
    private var indicatorObserver: AnyCancellable?

    init(model: AppModel) {
        self.model = model
        self.statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        self.settingsWindow = SettingsWindowController(model: model)
        if Self.hasUpdateFeed {
            self.updaterController = SPUStandardUpdaterController(
                startingUpdater: true,
                updaterDelegate: nil,
                userDriverDelegate: nil
            )
        } else {
            self.updaterController = nil
        }
        super.init()

        if let button = statusItem.button {
            button.target = self
            button.action = #selector(handleClick)
            // 必须显式声明左右键都触发 action,否则右键不会回调。
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
        refreshIcon()

        // `AppModel.indicator` 是 computed,没有自己的 publisher;订阅 objectWillChange
        // 后经主队列延一拍再读,否则读到的还是变更前的值。
        indicatorObserver = model.objectWillChange
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in self?.refreshIcon() }
    }

    // MARK: - 图标

    private func refreshIcon() {
        guard let button = statusItem.button else { return }
        let indicator = model.indicator
        button.image = MenuBarIconImage.image(dotColor: Self.dotColor(for: indicator))
        button.toolTip = "Sumpter · \(indicator.label)"
        button.setAccessibilityLabel("Sumpter · \(indicator.label)")
    }

    static func dotColor(for indicator: StatusIndicator) -> NSColor {
        switch indicator.dotStyle {
        case .ok: .systemGreen
        case .neutral: .systemGray
        case .warn: .systemOrange
        case .fault: .systemRed
        case .quiet: .tertiaryLabelColor
        case .starting: .systemBlue
        }
    }

    // MARK: - 点击分流

    @objc private func handleClick() {
        // Control+左键 在 macOS 上等同右键,一并归到次级操作。
        let event = NSApp.currentEvent
        let isSecondary = event?.type == .rightMouseUp
            || event?.modifierFlags.contains(.control) == true
        if isSecondary {
            presentMenu()
        } else {
            settingsWindow.show()
        }
    }

    /// 右键菜单。不能把菜单常驻在 `statusItem.menu` 上——那样左键也会弹菜单、
    /// 且 action 根本不会被调用。所以临时挂上、立刻点开、随后清空。
    private func presentMenu() {
        statusItem.menu = buildMenu()
        statusItem.button?.performClick(nil)
        statusItem.menu = nil
    }

    /// 每次右键都重建,好让「启动/停止」标题与禁用态反映当前状态。
    private func buildMenu() -> NSMenu {
        let menu = NSMenu()
        menu.addItem(menuItem(title: "打开主面板", action: #selector(menuOpenSettings)))
        menu.addItem(
            menuItem(
                title: toggleTitle,
                action: #selector(menuToggleProxy),
                enabled: !model.toggleInFlight
            )
        )
        menu.addItem(.separator())
        menu.addItem(menuItem(title: "刷新状态", action: #selector(menuRefresh)))
        menu.addItem(menuItem(title: "重新加载配置", action: #selector(menuReloadConfig)))
        menu.addItem(
            menuItem(
                title: "检查更新…",
                action: #selector(menuCheckForUpdates),
                enabled: updaterController != nil
            )
        )
        menu.addItem(menuItem(title: "打开配置目录", action: #selector(menuOpenConfigDirectory)))
        menu.addItem(.separator())
        menu.addItem(menuItem(title: "退出Sumpter", action: #selector(menuQuit)))
        return menu
    }

    private func menuItem(title: String, action: Selector, enabled: Bool = true) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
        item.target = self
        item.isEnabled = enabled
        return item
    }

    private var toggleTitle: String {
        if model.toggleInFlight {
            return model.sidecarState.isActive ? "停止中…" : "启动中…"
        }
        switch model.sidecarState {
        case .running, .starting, .unreachable: return "停止运行"
        case .stopped, .crashed: return "启动运行"
        }
    }

    // MARK: - 菜单动作

    @objc private func menuOpenSettings() { settingsWindow.show() }
    @objc private func menuToggleProxy() { model.toggleProxy() }
    @objc private func menuRefresh() { model.refresh() }
    @objc private func menuReloadConfig() { model.reloadConfigFromDisk() }
    @objc private func menuCheckForUpdates() { updaterController?.checkForUpdates(nil) }
    @objc private func menuOpenConfigDirectory() { model.openConfigDirectory() }
    @objc private func menuQuit() { model.shutdownAndQuit() }

    private static var hasUpdateFeed: Bool {
        guard let rawURL = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String,
              !rawURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              URL(string: rawURL) != nil,
              let publicKey = Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") as? String,
              !publicKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return false
        }
        return true
    }
}

/// 主设置窗口。由 AppKit 持有而不是 SwiftUI `Window` scene:状态项的点击回调在
/// AppKit 上下文里,拿不到 `@Environment(\.openWindow)`,而 `SettingsRootView`
/// 本身不依赖任何 Scene 环境值,放进 `NSHostingView` 即可。
@MainActor
final class SettingsWindowController: NSObject, NSWindowDelegate {
    private static let frameAutosaveName = "Sumpter.SettingsWindow"
    // These are preferred sizes, not unconditional frames.  The actual first
    // frame is resolved against the current screen's visibleFrame below so a
    // menu bar, Dock, or a short laptop display can never hide the title bar
    // or the bottom resize affordance.
    /// Screenshot target is 2620×2014 pixels on a 2× display (about
    /// 1310×1007pt including the title bar).  This is the content-area size;
    /// the resulting window frame is therefore close to the supplied image
    /// while staying compact on first launch.
    nonisolated static let preferredContentSize = NSSize(width: 1_310, height: 960)
    private static let preferredMinimumContentSize = NSSize(width: 760, height: 560)
    private static let screenInset = CGSize(width: 32, height: 48)

    private let model: AppModel
    private let windowVisibility = SumpterWindowVisibility()
    private var window: NSWindow?
    private var lifecycleState: AppWindowLifecycleState = .accessory

    init(model: AppModel) {
        self.model = model
        super.init()
    }

    func show() {
        let window = window ?? makeWindow()
        self.window = window
        windowVisibility.isVisible = true
        if model.statisticsVisible {
            model.refreshStatisticsIfNeeded(force: true)
        }
        // LSUIElement apps normally use the accessory policy and therefore do
        // not participate in Cmd+Tab.  A user-visible settings window is a
        // regular app window; switching to another app must leave this policy
        // intact so Cmd+Tab can return here.
        apply(.opened)
        // LSUIElement 应用没有 Dock 图标,不主动激活的话窗口会开在其它 app 后面。
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    func hide() {
        windowVisibility.isVisible = false
        window?.orderOut(nil)
        restoreAccessoryPolicy()
    }

    func windowWillClose(_ notification: Notification) {
        windowVisibility.isVisible = false
        restoreAccessoryPolicy()
    }

    private func restoreAccessoryPolicy() {
        apply(.closed)
    }

    private func apply(_ event: AppWindowLifecycleEvent) {
        lifecycleState = lifecycleState.applying(event)
        NSApp.setActivationPolicy(lifecycleState == .regular ? .regular : .accessory)
    }

    private func makeWindow() -> NSWindow {
        let targetScreen = Self.screen(containing: NSEvent.mouseLocation)
            ?? NSScreen.main
            ?? NSScreen.screens.first
        let visibleFrame = targetScreen?.visibleFrame ?? NSRect(
            origin: .zero,
            size: Self.preferredContentSize
        )
        let initialSize = Self.fittingContentSize(
            preferred: Self.preferredContentSize,
            visibleFrame: visibleFrame
        )
        let minimumSize = Self.fittingContentSize(
            preferred: Self.preferredMinimumContentSize,
            visibleFrame: visibleFrame
        )
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: initialSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Sumpter设置"
        window.contentView = NSHostingView(
            rootView: SettingsRootView(model: model, windowVisibility: windowVisibility)
        )
        window.contentMinSize = minimumSize
        // `setFrameUsingName` reports whether a saved frame actually existed;
        // `setFrameAutosaveName` only registers future persistence and its
        // Boolean describes name registration, not restoration.  Keeping the
        // two calls separate prevents a first-launch window from staying at
        // the zero origin and avoids overwriting a user's saved layout.
        let restored = window.setFrameUsingName(Self.frameAutosaveName)
        _ = window.setFrameAutosaveName(Self.frameAutosaveName)
        if !restored {
            window.setContentSize(initialSize)
            let frame = window.frame
            let centered = NSRect(
                x: visibleFrame.midX - frame.width / 2,
                y: visibleFrame.midY - frame.height / 2,
                width: frame.width,
                height: frame.height
            )
            window.setFrame(
                Self.constrainedFrame(centered, visibleFrame: visibleFrame),
                display: false
            )
        }
        // A saved frame may belong to a display that is no longer connected or
        // may exceed the current visible area after a Dock/menu-bar change.
        // Clamp it without changing its size unless the visible frame requires
        // it, then let autosave remember the corrected frame.
        let frameScreen = Self.screen(containing: NSPoint(
            x: window.frame.midX,
            y: window.frame.midY
        ))
            ?? targetScreen
        if let frameScreen {
            let constrained = Self.constrainedFrame(
                window.frame,
                visibleFrame: frameScreen.visibleFrame
            )
            if constrained != window.frame {
                window.setFrame(constrained, display: false)
            }
        }
        window.delegate = self
        // 关闭窗口后保留实例,下次点击复用同一个,避免每次重建整棵视图树。
        window.isReleasedWhenClosed = false
        return window
    }

    /// Resolve a preferred content size while reserving a small frame/title
    /// bar inset.  Kept as a pure helper so the sizing contract can be tested
    /// without constructing an NSWindow.
    nonisolated static func fittingContentSize(
        preferred: NSSize,
        visibleFrame: NSRect,
        inset: CGSize = CGSize(width: 32, height: 48)
    ) -> NSSize {
        let availableWidth = max(1, visibleFrame.width - inset.width)
        let availableHeight = max(1, visibleFrame.height - inset.height)
        return NSSize(
            width: min(preferred.width, availableWidth),
            height: min(preferred.height, availableHeight)
        )
    }

    /// Keep a restored frame wholly inside one display's visible area.  The
    /// operation is deliberately recoverable: it moves and shrinks only the
    /// window frame, never touching app state or user configuration.
    nonisolated static func constrainedFrame(_ frame: NSRect, visibleFrame: NSRect) -> NSRect {
        var result = frame
        result.size.width = min(result.width, visibleFrame.width)
        result.size.height = min(result.height, visibleFrame.height)
        result.origin.x = min(
            max(result.origin.x, visibleFrame.minX),
            visibleFrame.maxX - result.width
        )
        result.origin.y = min(
            max(result.origin.y, visibleFrame.minY),
            visibleFrame.maxY - result.height
        )
        return result
    }

    private static func screen(containing point: NSPoint) -> NSScreen? {
        NSScreen.screens.first { $0.frame.contains(point) }
    }
}

/// 菜单栏图标:线描驴头 + 右下角状态点。由 StatusItemController 绘制到状态项按钮。
enum MenuBarIconImage {
    static func image(dotColor: NSColor) -> NSImage {
        let size = NSSize(width: 22, height: 18)
        let image = NSImage(size: size, flipped: false) { rect in
            let scaleX = rect.width / size.width
            let scaleY = rect.height / size.height
            func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
                NSPoint(x: rect.minX + x * scaleX, y: rect.minY + y * scaleY)
            }
            func box(_ x: CGFloat, _ y: CGFloat, _ width: CGFloat, _ height: CGFloat) -> NSRect {
                NSRect(x: rect.minX + x * scaleX, y: rect.minY + y * scaleY, width: width * scaleX, height: height * scaleY)
            }

            let stroke = NSColor.labelColor
            stroke.setStroke()

            let ears = NSBezierPath()
            ears.lineWidth = 1.45
            ears.lineCapStyle = .round
            ears.lineJoinStyle = .round
            ears.move(to: point(7.1, 11.5))
            ears.line(to: point(5.7, 16.0))
            ears.line(to: point(9.0, 12.8))
            ears.move(to: point(13.0, 12.8))
            ears.line(to: point(16.3, 16.0))
            ears.line(to: point(14.9, 11.5))
            ears.stroke()

            let head = NSBezierPath(roundedRect: box(6.2, 3.4, 9.6, 10.7), xRadius: 4.9 * scaleX, yRadius: 5.2 * scaleY)
            head.lineWidth = 1.55
            head.stroke()

            let muzzle = NSBezierPath()
            muzzle.lineWidth = 1.25
            muzzle.lineCapStyle = .round
            muzzle.move(to: point(9.0, 6.1))
            muzzle.curve(to: point(13.0, 6.1), controlPoint1: point(9.8, 5.1), controlPoint2: point(12.2, 5.1))
            muzzle.stroke()

            stroke.setFill()
            NSBezierPath(ovalIn: box(8.6, 9.0, 1.15, 1.15)).fill()
            NSBezierPath(ovalIn: box(12.25, 9.0, 1.15, 1.15)).fill()

            let dotRect = box(15.1, 2.0, 4.9, 4.9)
            NSColor.windowBackgroundColor.setFill()
            NSBezierPath(ovalIn: dotRect.insetBy(dx: -0.8 * scaleX, dy: -0.8 * scaleY)).fill()
            dotColor.setFill()
            NSBezierPath(ovalIn: dotRect).fill()

            return true
        }
        image.isTemplate = false
        return image
    }
}
