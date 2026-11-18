import SwiftUI

/// Shared semantic palette for the macOS shell.  The values intentionally
/// mirror the Linux WebUI's semantic tokens (canvas, surfaces, text, brand
/// and status colors) across both the neutral cobalt-blue defaults and the
/// Catppuccin palette flavors while leaving typography and controls to macOS.
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

    struct Palette: Sendable {
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

    private struct CatppuccinFlavorSpec: Sendable {
        let isLight: Bool
        let canvas: UInt32
        let sidebar: UInt32
        let surface: UInt32
        let raised: UInt32
        let inset: UInt32
        let borderSubtle: UInt32?
        let borderMedium: UInt32?
        let borderStrong: UInt32?
        let textPrimary: UInt32
        let textSecondary: UInt32
        let textMuted: UInt32
        let textInverse: UInt32
        let info: UInt32
        let success: UInt32
        let warning: UInt32
        let danger: UInt32
        let unknown: UInt32

        func makePalette(brandHex: UInt32) -> Palette {
            let brand = Color(hex: brandHex)
            let primaryText = Color(hex: textPrimary)
            return Palette(
                canvas: Color(hex: canvas),
                sidebar: Color(hex: sidebar),
                surface: Color(hex: surface),
                raised: Color(hex: raised),
                inset: Color(hex: inset),
                hover: isLight ? Color.black.opacity(0.045) : primaryText.opacity(0.06),
                active: brand.opacity(isLight ? 0.11 : 0.16),
                borderSubtle: borderSubtle.map(Color.init(hex:)) ?? primaryText.opacity(0.09),
                borderMedium: borderMedium.map(Color.init(hex:)) ?? primaryText.opacity(0.16),
                borderStrong: borderStrong.map(Color.init(hex:)) ?? primaryText.opacity(0.24),
                textPrimary: primaryText,
                textSecondary: Color(hex: textSecondary),
                textMuted: Color(hex: textMuted),
                textInverse: Color(hex: textInverse),
                brand: brand,
                brandHover: brand.opacity(isLight ? 0.86 : 0.88),
                info: Color(hex: info),
                success: Color(hex: success),
                warning: Color(hex: warning),
                danger: Color(hex: danger),
                unknown: Color(hex: unknown)
            )
        }
    }

    private static let latteSpec = CatppuccinFlavorSpec(
        isLight: true,
        canvas: 0xEFF1F5,
        sidebar: 0xE6E9EF,
        surface: 0xF9FAFD,
        raised: 0xFCFCFE,
        inset: 0xE6E9EF,
        borderSubtle: 0xDCE0E8,
        borderMedium: 0xCCD0DA,
        borderStrong: 0xACB0BE,
        textPrimary: 0x4C4F69,
        textSecondary: 0x5C5F77,
        textMuted: 0x6C6F85,
        textInverse: 0xFFFFFF,
        info: 0x209FB5,
        success: 0x40A02B,
        warning: 0xDF8E1D,
        danger: 0xD20F39,
        unknown: 0x7C7F93
    )

    private static let frappeSpec = CatppuccinFlavorSpec(
        isLight: false,
        canvas: 0x303446,
        sidebar: 0x292C3C,
        surface: 0x363A4E,
        raised: 0x3B4056,
        inset: 0x292C3C,
        borderSubtle: nil,
        borderMedium: nil,
        borderStrong: nil,
        textPrimary: 0xC6D0F5,
        textSecondary: 0xB5BFE2,
        textMuted: 0xA5ADCE,
        textInverse: 0x232634,
        info: 0x85C1DC,
        success: 0xA6D189,
        warning: 0xE5C890,
        danger: 0xE78284,
        unknown: 0x949CBB
    )

    private static let macchiatoSpec = CatppuccinFlavorSpec(
        isLight: false,
        canvas: 0x24273A,
        sidebar: 0x1E2030,
        surface: 0x2B2F45,
        raised: 0x31354D,
        inset: 0x1E2030,
        borderSubtle: nil,
        borderMedium: nil,
        borderStrong: nil,
        textPrimary: 0xCAD3F5,
        textSecondary: 0xB8C0E0,
        textMuted: 0xA5ADCE,
        textInverse: 0x181926,
        info: 0x7DC4E4,
        success: 0xA6DA95,
        warning: 0xEED49F,
        danger: 0xED8796,
        unknown: 0x939AB7
    )

    private static let mochaSpec = CatppuccinFlavorSpec(
        isLight: false,
        canvas: 0x1E1E2E,
        sidebar: 0x181825,
        surface: 0x252538,
        raised: 0x2A2B40,
        inset: 0x181825,
        borderSubtle: nil,
        borderMedium: nil,
        borderStrong: nil,
        textPrimary: 0xCDD6F4,
        textSecondary: 0xBAC2DE,
        textMuted: 0xA6ADC8,
        textInverse: 0x11111B,
        info: 0x74C7EC,
        success: 0xA6E3A1,
        warning: 0xF9E2AF,
        danger: 0xF38BA8,
        unknown: 0x9399B2
    )

    static func palette(for scheme: ColorScheme) -> Palette {
        scheme == .dark ? .dark : .light
    }

    static func palette(
        mode: SumpterAppearanceMode,
        systemScheme: ColorScheme,
        accent: SumpterCatppuccinAccent = .mauve
    ) -> Palette {
        switch mode {
        case .system:
            return palette(for: systemScheme)
        case .light:
            return .light
        case .dark:
            return .dark
        case .catppuccinLatte:
            return latteSpec.makePalette(brandHex: accent.hexValue(for: .catppuccinLatte))
        case .catppuccinFrappe:
            return frappeSpec.makePalette(brandHex: accent.hexValue(for: .catppuccinFrappe))
        case .catppuccinMacchiato:
            return macchiatoSpec.makePalette(brandHex: accent.hexValue(for: .catppuccinMacchiato))
        case .catppuccinMocha:
            return mochaSpec.makePalette(brandHex: accent.hexValue(for: .catppuccinMocha))
        }
    }
}

/// Official Catppuccin accent options mirrored across Linux WebUI and macOS.
enum SumpterCatppuccinAccent: String, CaseIterable, Identifiable, Sendable {
    case mauve
    case blue
    case sapphire
    case teal
    case green
    case peach
    case pink
    case lavender

    var id: String { rawValue }

    var title: String {
        switch self {
        case .mauve: "丁香紫 (Mauve)"
        case .blue: "晴空蓝 (Blue)"
        case .sapphire: "宝石蓝 (Sapphire)"
        case .teal: "青碧色 (Teal)"
        case .green: "鼠尾草绿 (Green)"
        case .peach: "蜜桃橙 (Peach)"
        case .pink: "樱花粉 (Pink)"
        case .lavender: "薰衣草 (Lavender)"
        }
    }

    var shortTitle: String {
        switch self {
        case .mauve: "丁香紫"
        case .blue: "晴空蓝"
        case .sapphire: "宝石蓝"
        case .teal: "青碧色"
        case .green: "鼠尾草绿"
        case .peach: "蜜桃橙"
        case .pink: "樱花粉"
        case .lavender: "薰衣草"
        }
    }

    func hexValue(for mode: SumpterAppearanceMode) -> UInt32 {
        switch (self, mode) {
        case (.mauve, .catppuccinLatte): 0x8839EF
        case (.mauve, .catppuccinFrappe): 0xCA9EE6
        case (.mauve, .catppuccinMacchiato): 0xC6A0F6
        case (.mauve, _): 0xCBA6F7

        case (.blue, .catppuccinLatte): 0x1E66F5
        case (.blue, .catppuccinFrappe): 0x8CAAEE
        case (.blue, .catppuccinMacchiato): 0x8AADF4
        case (.blue, _): 0x89B4FA

        case (.sapphire, .catppuccinLatte): 0x209FB5
        case (.sapphire, .catppuccinFrappe): 0x85C1DC
        case (.sapphire, .catppuccinMacchiato): 0x7DC4E4
        case (.sapphire, _): 0x74C7EC

        case (.teal, .catppuccinLatte): 0x179299
        case (.teal, .catppuccinFrappe): 0x81C8BE
        case (.teal, .catppuccinMacchiato): 0x8BD5CA
        case (.teal, _): 0x94E2D5

        case (.green, .catppuccinLatte): 0x40A02B
        case (.green, .catppuccinFrappe): 0xA6D189
        case (.green, .catppuccinMacchiato): 0xA6DA95
        case (.green, _): 0xA6E3A1

        case (.peach, .catppuccinLatte): 0xFE640B
        case (.peach, .catppuccinFrappe): 0xEF9F76
        case (.peach, .catppuccinMacchiato): 0xF5A97F
        case (.peach, _): 0xFAB387

        case (.pink, .catppuccinLatte): 0xEA76CB
        case (.pink, .catppuccinFrappe): 0xF4B8E4
        case (.pink, .catppuccinMacchiato): 0xF5BDE6
        case (.pink, _): 0xF5C2E7

        case (.lavender, .catppuccinLatte): 0x7287FD
        case (.lavender, .catppuccinFrappe): 0xBABBF1
        case (.lavender, .catppuccinMacchiato): 0xB4BEFE
        case (.lavender, _): 0xB4BEFE
        }
    }

    func color(for mode: SumpterAppearanceMode) -> Color {
        Color(hex: hexValue(for: mode))
    }
}

/// AppKit 状态栏应用的设置窗口会被复用并隐藏。把可见性放进一个轻量
/// 的独立观察对象，避免让动画组件继续在 orderOut 后驱动 SwiftUI 重绘。
final class SumpterWindowVisibility: ObservableObject {
    @Published var isVisible = false
}

/// Local appearance preference.  “跟随系统” keeps the normal macOS behavior;
/// explicit neutral and Catppuccin flavors match the Linux WebUI theme picker.
enum SumpterAppearanceMode: String, CaseIterable, Identifiable, Sendable {
    case system
    case light
    case dark
    case catppuccinLatte = "catppuccin-latte"
    case catppuccinFrappe = "catppuccin-frappe"
    case catppuccinMacchiato = "catppuccin-macchiato"
    case catppuccinMocha = "catppuccin-mocha"

    static let classicCases: [SumpterAppearanceMode] = [.system, .light, .dark]
    static let catppuccinCases: [SumpterAppearanceMode] = [
        .catppuccinLatte,
        .catppuccinFrappe,
        .catppuccinMacchiato,
        .catppuccinMocha,
    ]

    var id: String { rawValue }

    var isCatppuccin: Bool {
        switch self {
        case .catppuccinLatte, .catppuccinFrappe, .catppuccinMacchiato, .catppuccinMocha:
            true
        case .system, .light, .dark:
            false
        }
    }

    var title: String {
        switch self {
        case .system: "跟随系统"
        case .light: "纯白浅色"
        case .dark: "曜石深色"
        case .catppuccinLatte: "Latte · 拿铁"
        case .catppuccinFrappe: "Frappé · 星冰乐"
        case .catppuccinMacchiato: "Macchiato · 玛奇朵"
        case .catppuccinMocha: "Mocha · 摩卡"
        }
    }

    var shortTitle: String {
        switch self {
        case .system: "跟随系统"
        case .light: "纯白"
        case .dark: "曜石"
        case .catppuccinLatte: "Latte"
        case .catppuccinFrappe: "Frappé"
        case .catppuccinMacchiato: "Macchiato"
        case .catppuccinMocha: "Mocha"
        }
    }

    var icon: String {
        switch self {
        case .system: "circle.lefthalf.filled"
        case .light: "sun.max"
        case .dark: "moon"
        case .catppuccinLatte: "cup.and.saucer"
        case .catppuccinFrappe: "cloud.moon"
        case .catppuccinMacchiato: "moon.haze"
        case .catppuccinMocha: "moon.stars"
        }
    }

    var preferredColorScheme: ColorScheme? {
        switch self {
        case .system:
            nil
        case .light, .catppuccinLatte:
            .light
        case .dark, .catppuccinFrappe, .catppuccinMacchiato, .catppuccinMocha:
            .dark
        }
    }

    static func modeAfterSelectingCatppuccinAccent(
        currentMode: SumpterAppearanceMode,
        resolvedScheme: ColorScheme
    ) -> SumpterAppearanceMode {
        if currentMode.isCatppuccin {
            return currentMode
        }
        return resolvedScheme == .dark ? .catppuccinMocha : .catppuccinLatte
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
