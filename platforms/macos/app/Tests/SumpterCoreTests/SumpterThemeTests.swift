import SwiftUI
import XCTest
@testable import SumpterApp

final class SumpterThemeTests: XCTestCase {
    func testAppearanceModesMirrorClassicAndCatppuccinFlavors() {
        XCTAssertEqual(
            SumpterAppearanceMode.allCases.map(\.rawValue),
            [
                "system",
                "light",
                "dark",
                "catppuccin-latte",
                "catppuccin-frappe",
                "catppuccin-macchiato",
                "catppuccin-mocha",
            ]
        )

        XCTAssertNil(SumpterAppearanceMode.system.preferredColorScheme)
        XCTAssertEqual(SumpterAppearanceMode.light.preferredColorScheme, .light)
        XCTAssertEqual(SumpterAppearanceMode.catppuccinLatte.preferredColorScheme, .light)
        XCTAssertEqual(SumpterAppearanceMode.dark.preferredColorScheme, .dark)
        XCTAssertEqual(SumpterAppearanceMode.catppuccinFrappe.preferredColorScheme, .dark)
        XCTAssertEqual(SumpterAppearanceMode.catppuccinMacchiato.preferredColorScheme, .dark)
        XCTAssertEqual(SumpterAppearanceMode.catppuccinMocha.preferredColorScheme, .dark)

        XCTAssertFalse(SumpterAppearanceMode.system.isCatppuccin)
        XCTAssertFalse(SumpterAppearanceMode.light.isCatppuccin)
        XCTAssertFalse(SumpterAppearanceMode.dark.isCatppuccin)
        XCTAssertTrue(SumpterAppearanceMode.catppuccinLatte.isCatppuccin)
        XCTAssertTrue(SumpterAppearanceMode.catppuccinMocha.isCatppuccin)
    }

    func testDefaultNeutralPalettesRemainIntact() {
        let lightPalette = SumpterTheme.palette(mode: .light, systemScheme: .dark, accent: .mauve)
        let darkPalette = SumpterTheme.palette(mode: .dark, systemScheme: .light, accent: .mauve)
        let systemDark = SumpterTheme.palette(mode: .system, systemScheme: .dark, accent: .pink)
        let systemLight = SumpterTheme.palette(mode: .system, systemScheme: .light, accent: .pink)

        XCTAssertEqual(lightPalette.canvas, SumpterTheme.Palette.light.canvas)
        XCTAssertEqual(lightPalette.brand, SumpterTheme.Palette.light.brand)
        XCTAssertEqual(darkPalette.canvas, SumpterTheme.Palette.dark.canvas)
        XCTAssertEqual(darkPalette.brand, SumpterTheme.Palette.dark.brand)
        XCTAssertEqual(systemDark.canvas, SumpterTheme.Palette.dark.canvas)
        XCTAssertEqual(systemLight.canvas, SumpterTheme.Palette.light.canvas)
    }

    func testCatppuccinFlavorsAndAccentsResolveExpectedColors() {
        XCTAssertEqual(SumpterCatppuccinAccent.allCases.count, 8)
        XCTAssertEqual(SumpterCatppuccinAccent.mauve.hexValue(for: .catppuccinLatte), 0x8839EF)
        XCTAssertEqual(SumpterCatppuccinAccent.mauve.hexValue(for: .catppuccinFrappe), 0xCA9EE6)
        XCTAssertEqual(SumpterCatppuccinAccent.mauve.hexValue(for: .catppuccinMacchiato), 0xC6A0F6)
        XCTAssertEqual(SumpterCatppuccinAccent.mauve.hexValue(for: .catppuccinMocha), 0xCBA6F7)
        XCTAssertEqual(SumpterCatppuccinAccent.blue.hexValue(for: .catppuccinMocha), 0x89B4FA)
        XCTAssertEqual(SumpterCatppuccinAccent.sapphire.hexValue(for: .catppuccinLatte), 0x209FB5)
        XCTAssertEqual(SumpterCatppuccinAccent.teal.hexValue(for: .catppuccinMocha), 0x94E2D5)
        XCTAssertEqual(SumpterCatppuccinAccent.peach.hexValue(for: .catppuccinLatte), 0xFE640B)
        XCTAssertEqual(SumpterCatppuccinAccent.pink.hexValue(for: .catppuccinFrappe), 0xF4B8E4)

        let mochaBlue = SumpterTheme.palette(mode: .catppuccinMocha, systemScheme: .light, accent: .blue)
        let latteMauve = SumpterTheme.palette(mode: .catppuccinLatte, systemScheme: .dark, accent: .mauve)
        XCTAssertEqual(mochaBlue.brand, SumpterCatppuccinAccent.blue.color(for: .catppuccinMocha))
        XCTAssertEqual(latteMauve.brand, SumpterCatppuccinAccent.mauve.color(for: .catppuccinLatte))
        XCTAssertEqual(latteMauve.textInverse, Color(red: 1, green: 1, blue: 1))
    }

    func testSelectingCatppuccinAccentEntersMatchingPolarityFlavorWhenNeeded() {
        XCTAssertEqual(
            SumpterAppearanceMode.modeAfterSelectingCatppuccinAccent(currentMode: .light, resolvedScheme: .light),
            .catppuccinLatte
        )
        XCTAssertEqual(
            SumpterAppearanceMode.modeAfterSelectingCatppuccinAccent(currentMode: .dark, resolvedScheme: .dark),
            .catppuccinMocha
        )
        XCTAssertEqual(
            SumpterAppearanceMode.modeAfterSelectingCatppuccinAccent(currentMode: .system, resolvedScheme: .dark),
            .catppuccinMocha
        )
        XCTAssertEqual(
            SumpterAppearanceMode.modeAfterSelectingCatppuccinAccent(currentMode: .catppuccinFrappe, resolvedScheme: .dark),
            .catppuccinFrappe
        )
        XCTAssertEqual(
            SumpterAppearanceMode.modeAfterSelectingCatppuccinAccent(currentMode: .catppuccinMacchiato, resolvedScheme: .dark),
            .catppuccinMacchiato
        )
    }
}
