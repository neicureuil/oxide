import AppKit
import OxideCore
import SwiftUI

/// A picked screen or window.
struct ScreenChoice {
    let source: ScreenSourceView
}

extension ScreenSourceView {
    /// Screens and windows have their own id spaces.
    var key: String { "\(kind == .window ? "w" : "s")\(id)" }
}

/// Screens and windows that can be shared, with thumbnails captured in the background.
@MainActor
@Observable
final class DesktopSources {
    private(set) var screens: [ScreenSourceView] = []
    private(set) var windows: [ScreenSourceView] = []
    private(set) var thumbnails: [String: NSImage] = [:]
    @ObservationIgnored private var loading: Task<Void, Never>?

    func load() {
        loading?.cancel()
        let own = Bundle.main.localizedInfoDictionary?["CFBundleName"] as? String ?? ProcessInfo.processInfo.processName
        loading = Task.detached(priority: .userInitiated) { [weak self] in
            let screens = mediaScreenSources(kind: .screen)
            let windows = mediaScreenSources(kind: .window).filter { source in
                let title = source.title.trimmingCharacters(in: .whitespaces)
                return !title.isEmpty && title != own
            }
            await MainActor.run {
                self?.screens = screens
                self?.windows = windows
            }
            // One at a time, screens first: each takes a capture.
            for source in screens + windows {
                guard !Task.isCancelled else { return }
                guard let thumbnail = mediaScreenThumbnail(source: source, maxWidth: 320),
                      let image = Self.image(thumbnail) else { continue }
                await MainActor.run { self?.thumbnails[source.key] = image }
            }
        }
    }

    func close() {
        loading?.cancel()
        loading = nil
        screens = []
        windows = []
        thumbnails = [:]
    }

    nonisolated private static func image(_ thumbnail: ThumbnailView) -> NSImage? {
        let width = Int(thumbnail.width), height = Int(thumbnail.height)
        guard width > 0, height > 0, thumbnail.rgba.count >= width * height * 4,
              let provider = CGDataProvider(data: Data(thumbnail.rgba) as CFData),
              let image = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                                  bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                                  provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { return nil }
        return NSImage(cgImage: image, size: NSSize(width: width, height: height))
    }
}

/// Share sheet: pick a screen or a window, then share it.
struct ScreenSharePicker: View {
    let share: (ScreenChoice) -> Void
    let cancel: () -> Void
    @State private var sources = DesktopSources()
    @State private var selected: String?
    @State private var allowed = CGPreflightScreenCaptureAccess()

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(L("ui.screen_share_picker.share_screen")).font(.figtree(18, .semibold))
            if allowed {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        section(L("ui.screen_share_picker.screens"), sources.screens)
                        if !sources.windows.isEmpty { section(L("ui.screen_share_picker.windows"), sources.windows) }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                Muted(L("ui.screen_share_picker.oxide_must_be_allowed_in_system"), size: 13)
                Button(L("ui.screen_share_picker.open_settings")) {
                    _ = CGRequestScreenCaptureAccess()
                    NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")!)
                }
                .buttonStyle(OutlineButtonStyle())
                Spacer()
            }
            HStack {
                Spacer()
                Button(L("ui.screen_share_picker.cancel"), action: cancel).buttonStyle(OutlineButtonStyle()).keyboardShortcut(.cancelAction)
                Button(L("ui.screen_share_picker.share")) {
                    guard let chosen else { return }
                    share(ScreenChoice(source: chosen))
                }
                .buttonStyle(PrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(!allowed || chosen == nil)
            }
        }
        .padding(22)
        .frame(width: 640, height: 520)
        .onAppear {
            if !allowed { allowed = CGRequestScreenCaptureAccess() }
            guard allowed else { return }
            sources.load()
        }
        .onChange(of: sources.screens) { _, screens in
            if selected == nil { selected = screens.first?.key }
        }
        .onDisappear {
            sources.close()
        }
    }

    private var chosen: ScreenSourceView? {
        (sources.screens + sources.windows).first { $0.key == selected }
    }

    /// Screens come without a name: number them.
    private func displayName(of source: ScreenSourceView, in items: [ScreenSourceView]) -> String {
        guard source.title.trimmingCharacters(in: .whitespaces).isEmpty else { return source.title }
        let number = (items.firstIndex { $0.key == source.key } ?? 0) + 1
        return L("ui.screen_share_picker.screen_number", ["n": number])
    }

    private func section(_ title: String, _ items: [ScreenSourceView]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.figtree(13, .semibold)).foregroundStyle(Theme.muted)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 170), spacing: 12)], spacing: 12) {
                ForEach(items, id: \.key) { source in
                    let isSelected = selected == source.key
                    Button { selected = source.key } label: {
                        VStack(spacing: 6) {
                            Group {
                                if let thumbnail = sources.thumbnails[source.key] {
                                    Image(nsImage: thumbnail).resizable().aspectRatio(contentMode: .fit)
                                } else {
                                    Image(systemName: source.kind == .window ? "macwindow" : "display")
                                        .font(.system(size: 28)).foregroundStyle(.white.opacity(0.7))
                                }
                            }
                            .frame(height: 96)
                            .frame(maxWidth: .infinity)
                            .background(Color.black.opacity(0.85), in: RoundedRectangle(cornerRadius: 8))
                            Text(displayName(of: source, in: items)).font(.figtree(12)).lineLimit(1).truncationMode(.middle)
                        }
                        .padding(6)
                        .background(isSelected ? Theme.accentSoft : Color.clear, in: RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(isSelected ? Theme.accent : Theme.border, lineWidth: isSelected ? 2 : 1))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .pointerCursor()
                    .accessibilityAddTraits(isSelected ? .isSelected : [])
                }
            }
        }
    }
}
