import SwiftUI

/// Shared semantic palette for the macOS shell.  The values intentionally
/// mirror the Linux WebUI's light/dark semantic tokens (canvas, surfaces,
/// text, brand and status colors) while leaving typography and controls to
/// the native macOS system.
enum SumpterTheme {
    /// Surface geometry shared with the Linux WebUI tokens.  These values
    /// describe semantic layers, not platform control sizes: native SwiftUI
    /// controls keep their macOS behavior while panels and metric cards align
    /// visually with the browser dashboard.
    enum Layout {
        static let panelRadius: CGFloat = 16
        static let panelPadding: CGFloat = 20
        static let panelSpacing: CGFloat = 16
        static let metricRadius: CGFloat = 14
        static let metricPadding: CGFloat = 16
        static let metricMinimumHeight: CGFloat = 96
    }

    struct Palette {
        let canvas: Color
        let sidebar: Color
        let surface: Color
        let raised: Color
        let inset: Color
        let hover: Color
        let active: Color
        let borderSubtle: Color
        let borderMedium: Color
        let borderStrong: Color
        let textPrimary: Color
        let textSecondary: Color
        let textMuted: Color
        let textInverse: Color
        let brand: Color
        let brandHover: Color
        let info: Color
        let success: Color
        let warning: Color
        let danger: Color
        let unknown: Color

        static let light = Palette(
            canvas: Color(hex: 0xF8F9FC),
            sidebar: Color(hex: 0xF2F4F8),
            surface: Color.white,
            raised: Color.white,
            inset: Color(hex: 0xF1F3F8),
            hover: Color.black.opacity(0.038),
            active: Color(hex: 0xCC4E26).opacity(0.10),
            borderSubtle: Color.black.opacity(0.065),
            borderMedium: Color.black.opacity(0.11),
            borderStrong: Color.black.opacity(0.18),
            textPrimary: Color(hex: 0x111520),
            textSecondary: Color(hex: 0x555E70),
            textMuted: Color(hex: 0x677083),
            textInverse: Color(hex: 0xFAFBFD),
            brand: Color(hex: 0xCC4E26),
            brandHover: Color(hex: 0xB3411D),
            info: Color(hex: 0x117A9B),
            success: Color(hex: 0x158554),
            warning: Color(hex: 0xC47610),
            danger: Color(hex: 0xDC3535),
            unknown: Color(hex: 0x667085)
        )

        static let dark = Palette(
            canvas: Color(hex: 0x0E1118),
            sidebar: Color(hex: 0x090B10).opacity(0.97),
            surface: Color(hex: 0x151923),
            raised: Color(hex: 0x1B202D),
            inset: Color(hex: 0x11151E),
            hover: Color.white.opacity(0.055),
            active: Color(hex: 0xF0784A).opacity(0.14),
            borderSubtle: Color.white.opacity(0.085),
            borderMedium: Color.white.opacity(0.13),
            borderStrong: Color.white.opacity(0.21),
            textPrimary: Color(hex: 0xF2F4F8),
            textSecondary: Color(hex: 0x97A0B2),
            textMuted: Color(hex: 0x7E889B),
            textInverse: Color(hex: 0x0E1118),
            brand: Color(hex: 0xF0784A),
            brandHover: Color(hex: 0xE06536),
            info: Color(hex: 0x38B6D6),
            success: Color(hex: 0x30B87A),
            warning: Color(hex: 0xEEAA34),
            danger: Color(hex: 0xF05D5E),
            unknown: Color(hex: 0x7E889B)
        )
    }

    static func palette(for scheme: ColorScheme) -> Palette {
        scheme == .dark ? .dark : .light
    }
}

/// AppKit 状态栏应用的设置窗口会被复用并隐藏。把可见性放进一个轻量
/// 的独立观察对象，避免让动画组件继续在 orderOut 后驱动 SwiftUI 重绘。
final class SumpterWindowVisibility: ObservableObject {
    @Published var isVisible = false
}

/// Local appearance preference.  “跟随系统” keeps the normal macOS behavior;
/// explicit light/dark modes make the shell visually match a Linux WebUI
/// workspace when the two are being compared side by side.
enum SumpterAppearanceMode: String, CaseIterable, Identifiable {
    case system
    case light
    case dark

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: "跟随系统"
        case .light: "浅色"
        case .dark: "深色"
        }
    }

    var icon: String {
        switch self {
        case .system: "circle.lefthalf.filled"
        case .light: "sun.max"
        case .dark: "moon"
        }
    }

    var preferredColorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

private struct SumpterPaletteKey: EnvironmentKey {
    static let defaultValue = SumpterTheme.Palette.light
}

private struct SumpterWindowVisibleKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    var sumpterPalette: SumpterTheme.Palette {
        get { self[SumpterPaletteKey.self] }
        set { self[SumpterPaletteKey.self] = newValue }
    }

    var sumpterWindowVisible: Bool {
        get { self[SumpterWindowVisibleKey.self] }
        set { self[SumpterWindowVisibleKey.self] = newValue }
    }
}

private extension Color {
    init(hex: UInt32) {
        self.init(
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255
        )
    }
}
