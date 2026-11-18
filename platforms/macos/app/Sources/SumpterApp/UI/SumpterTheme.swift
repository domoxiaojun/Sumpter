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
            canvas: Color(hex: 0xFAFAFA),
            sidebar: Color(hex: 0xF4F4F5),
            surface: Color.white,
            raised: Color.white,
            inset: Color(hex: 0xF3F3F5),
            hover: Color.black.opacity(0.04),
            active: Color(hex: 0x2563EB).opacity(0.10),
            borderSubtle: Color.black.opacity(0.07),
            borderMedium: Color.black.opacity(0.12),
            borderStrong: Color.black.opacity(0.20),
            textPrimary: Color(hex: 0x09090B),
            textSecondary: Color(hex: 0x52525B),
            textMuted: Color(hex: 0x71717A),
            textInverse: Color.white,
            brand: Color(hex: 0x2563EB),
            brandHover: Color(hex: 0x1D4ED8),
            info: Color(hex: 0x0891B2),
            success: Color(hex: 0x109856),
            warning: Color(hex: 0xD97706),
            danger: Color(hex: 0xE11D48),
            unknown: Color(hex: 0x71717A)
        )

        static let dark = Palette(
            canvas: Color(hex: 0x0B0B0D),
            sidebar: Color(hex: 0x070708).opacity(0.97),
            surface: Color(hex: 0x141417),
            raised: Color(hex: 0x1B1B1F),
            inset: Color(hex: 0x0F0F12),
            hover: Color.white.opacity(0.055),
            active: Color(hex: 0x3B82F6).opacity(0.15),
            borderSubtle: Color.white.opacity(0.09),
            borderMedium: Color.white.opacity(0.14),
            borderStrong: Color.white.opacity(0.22),
            textPrimary: Color(hex: 0xF4F4F5),
            textSecondary: Color(hex: 0xA1A1AA),
            textMuted: Color(hex: 0x82828B),
            textInverse: Color.white,
            brand: Color(hex: 0x3B82F6),
            brandHover: Color(hex: 0x60A5FA),
            info: Color(hex: 0x38BDF8),
            success: Color(hex: 0x22C55E),
            warning: Color(hex: 0xF59E0B),
            danger: Color(hex: 0xF43F5E),
            unknown: Color(hex: 0x82828B)
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
