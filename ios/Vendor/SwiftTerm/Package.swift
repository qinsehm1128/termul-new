// swift-tools-version:6.0
// Vendored from https://github.com/migueldeicaza/SwiftTerm @ 1.20.0
// The upstream package registers SwiftTermBuildInfoPlugin, which Xcode 27
// rejects during "Validate plug-in". This local package keeps the iOS library
// only and ships a static SwiftTermBuildInfo stub.

import PackageDescription

let package = Package(
    name: "SwiftTerm",
    platforms: [
        .iOS(.v14),
        .visionOS(.v1),
    ],
    products: [
        .library(name: "SwiftTerm", targets: ["SwiftTerm"]),
    ],
    targets: [
        .target(
            name: "SwiftTerm",
            path: "Sources/SwiftTerm",
            resources: [
                // Copy, do not `.process`: Xcode 27 CLI often lacks the Metal
                // toolchain, and SwiftTerm can compile this source at runtime.
                .copy("Apple/Metal/Shaders.metal"),
            ]
        ),
    ],
    swiftLanguageModes: [.v5]
)
