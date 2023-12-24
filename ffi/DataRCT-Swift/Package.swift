// swift-tools-version: 5.7
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let package = Package(
    name: "DataRCT",
    platforms: [
        .iOS(.v15),
        .macOS(.v12)
    ],
    products: [
        .library(
            name: "DataRCT",
            targets: ["DataRCT"]),
    ],
    dependencies: [],
    targets: [
        .target(
            name: "DataRCT",
            dependencies: ["DataRCTFFI"]
        ),

        .binaryTarget(
            name: "DataRCTFFI",
            path: "DataRCTFFI.xcframework"
        ),

        .testTarget(
            name: "DataRCTTests",
            dependencies: ["DataRCT"]),
    ]
)
