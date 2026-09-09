// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "SumpterRustShell",
    defaultLocalization: "zh-Hans",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .library(name: "SumpterCore", targets: ["SumpterCore"]),
        .executable(name: "SumpterApp", targets: ["SumpterApp"])
    ],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle.git", exact: "2.9.6")
    ],
    targets: [
        .target(name: "SumpterCore"),
        .executableTarget(
            name: "SumpterApp",
            dependencies: [
                "SumpterCore",
                .product(name: "Sparkle", package: "Sparkle")
            ],
            resources: [
                .copy("Resources/pi-project-attribution.ts"),
                .copy("Resources/gemini-sumpter-wrapper.mjs"),
                .copy("Resources/client-attribution.mjs"),
            ]
        ),
        .testTarget(
            name: "SumpterCoreTests",
            dependencies: [
                "SumpterCore",
                "SumpterApp",
                .product(name: "Sparkle", package: "Sparkle")
            ]
        )
    ]
)
