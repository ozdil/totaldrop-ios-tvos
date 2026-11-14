// swift-tools-version: 6.0
// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

import PackageDescription

let package = Package(
    name: "TotalDropIOS",
    defaultLocalization: "en",
    platforms: [
        .iOS(.v18),
    ],
    products: [
        .library(
            name: "TotalDropIOS",
            targets: ["TotalDropIOS"]
        ),
        .library(
            name: "ShareExtension",
            type: .dynamic,
            targets: ["ShareExtension"]
        ),
    ],
    targets: [
        .target(
            name: "TotalDropIOS",
            path: "Sources/TotalDropIOS",
            swiftSettings: [
                .swiftLanguageMode(.v6),
            ],
            linkerSettings: [
                .linkedFramework("SwiftUI"),
                .linkedFramework("UIKit"),
                .linkedFramework("BackgroundTasks"),
                .linkedFramework("CoreImage"),
                .linkedFramework("UniformTypeIdentifiers"),
            ]
        ),
        .target(
            name: "ShareExtension",
            path: "Sources/ShareExtension",
            swiftSettings: [
                .swiftLanguageMode(.v6),
            ],
            linkerSettings: [
                .linkedFramework("SwiftUI"),
                .linkedFramework("UIKit"),
                .linkedFramework("UniformTypeIdentifiers"),
            ]
        ),
        .testTarget(
            name: "TotalDropIOSTests",
            dependencies: ["TotalDropIOS"],
            path: "Tests/TotalDropIOSTests",
            swiftSettings: [
                .swiftLanguageMode(.v6),
            ]
        ),
    ]
)
