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
            canvas: Color(hex: 0xF6F8FC),
            sidebar: Color(hex: 0xF8FAFC),
            surface: Color.white.opacity(0.92),
            raised: Color.white,
            inset: Color(hex: 0xF8FAFC),
            hover: Color.black.opacity(0.04),
            active: Color(hex: 0x0284C7).opacity(0.08),
            borderSubtle: Color.black.opacity(0.06),
            borderMedium: Color.black.opacity(0.11),
            borderStrong: Color.black.opacity(0.20),
            textPrimary: Color(hex: 0x0F172A),
            textSecondary: Color(hex: 0x475569),
            textMuted: Color(hex: 0x5F6F84),
            textInverse: Color(hex: 0xF8FAFC),
            brand: Color(hex: 0x0284C7),
            brandHover: Color(hex: 0x0369A1),
            info: Color(hex: 0x0891B2),
            success: Color(hex: 0x059669),
            warning: Color(hex: 0xD97706),
            danger: Color(hex: 0xE11D48),
            unknown: Color(hex: 0x64748B)
        )

        static let dark = Palette(
            canvas: Color(hex: 0x080C14),
            sidebar: Color(hex: 0x0A0F1A).opacity(0.96),
            surface: Color(hex: 0x0F172A).opacity(0.90),
            raised: Color(hex: 0x16213A).opacity(0.96),
            inset: Color(hex: 0x0D1424).opacity(0.90),
            hover: Color.white.opacity(0.06),
            active: Color(hex: 0x38BDF8).opacity(0.12),
            borderSubtle: Color.white.opacity(0.07),
            borderMedium: Color.white.opacity(0.12),
            borderStrong: Color.white.opacity(0.20),
            textPrimary: Color(hex: 0xF8FAFC),
            textSecondary: Color(hex: 0x94A3B8),
            textMuted: Color(hex: 0x8290A5),
            textInverse: Color(hex: 0x0F172A),
            brand: Color(hex: 0x38BDF8),
            brandHover: Color(hex: 0x0EA5E9),
            info: Color(hex: 0x06B6D4),
            success: Color(hex: 0x10B981),
            warning: Color(hex: 0xF59E0B),
            danger: Color(hex: 0xF43F5E),
            unknown: Color(hex: 0x64748B)
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
