// swift-tools-version:6.0
// Native macOS shell over the Rust core. Build the core first: scripts/build-core.sh
import PackageDescription

let package = Package(
    name: "Oxide",
    platforms: [.macOS(.v14)],
    targets: [
        .binaryTarget(name: "OxideCoreFFI", path: "OxideCore.xcframework"),
        .target(
            name: "OxideCore",
            dependencies: ["OxideCoreFFI"],
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
                .linkedFramework("CoreFoundation"),
                // The call engine's libwebrtc (crates/teams-media); `-ObjC` keeps its codec factories' categories.
                .linkedLibrary("c++"),
                .linkedFramework("AVFoundation"),
                .linkedFramework("AudioToolbox"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("CoreMedia"),
                .linkedFramework("CoreVideo"),
                .linkedFramework("VideoToolbox"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("IOSurface"),
                .linkedFramework("IOKit"),
                .linkedFramework("Metal"),
                .linkedFramework("MetalKit"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("OpenGL"),
                .linkedFramework("AppKit"),
                .unsafeFlags(["-Xlinker", "-ObjC"]),
            ]
        ),
        .executableTarget(
            name: "Oxide",
            dependencies: ["OxideCore"]
        ),
        .testTarget(name: "OxideTests", dependencies: ["Oxide", "OxideCore"]),
    ],
    swiftLanguageModes: [.v5]
)
