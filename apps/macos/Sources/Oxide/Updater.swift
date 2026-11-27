import AppKit
import CryptoKit
import SwiftUI
import OxideCore

/// A published GitHub release of Oxide, from the repository's `releases/latest`.
struct Release: Decodable, Identifiable, Equatable {
    struct Asset: Decodable, Equatable {
        let name: String
        let browserDownloadUrl: URL
        /// "sha256:<hex>", computed by GitHub on upload.
        let digest: String?
    }

    let tagName: String
    let body: String?
    let htmlUrl: URL
    let assets: [Asset]

    var id: String { tagName }
    /// "0.2.0" from "v0.2.0".
    var version: String { tagName.hasPrefix("v") ? String(tagName.dropFirst()) : tagName }
    /// The app archive: `Oxide-<version>.zip` holding `Oxide.app` at its root. The release also carries the GPUI
    /// client's files (`Oxide-gpui-…`), never taken for this app.
    var archive: Asset? {
        assets.first { $0.name == "Oxide-\(version).zip" }
            ?? assets.first {
                $0.name.hasPrefix("Oxide-") && !$0.name.hasPrefix("Oxide-gpui-") && $0.name.hasSuffix(".zip")
            }
    }

    /// The notes as inline Markdown: headings become bold lines, the rest is kept as written.
    var notes: AttributedString {
        let text = (body ?? "")
            .replacingOccurrences(of: "\r\n", with: "\n")
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { line in
                let heading = line.drop { $0 == "#" }
                return heading.count < line.count ? "**\(heading.trimmingCharacters(in: .whitespaces))**" : String(line)
            }
            .joined(separator: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}

/// Checks GitHub for a newer release at launch and from Settings, and installs it in place of the running app.
@MainActor
@Observable
final class Updater {
    static let shared = Updater()

    enum State: Equatable {
        case idle, checking, upToDate
        case available(Release)
        case downloading(Double)
        case installing
        case failed(String)
    }

    private static let latestURL = URL(string: "https://api.github.com/repos/neicureuil/oxide/releases/latest")!
    private static let bundleIdentifier = "fr.neicureuil.oxide"

    var state = State.idle
    /// The release offered in the update sheet, at launch or from Settings.
    var prompt: Release?

    /// "0.1.0": the bundle's version, the core's under `swift run`.
    static var currentVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? appVersion()
    }

    /// Only a real Oxide.app checks: not `swift run`, nor the demo.
    var enabled: Bool {
        Bundle.main.bundleURL.pathExtension == "app" && Store.current?.isDemo != true
    }

    /// Whether the app can replace its own bundle: not translocated by Gatekeeper, in a folder it can write.
    var canInstall: Bool {
        let app = Bundle.main.bundleURL
        let fm = FileManager.default
        return !app.path.contains("/AppTranslocation/")
            && fm.isWritableFile(atPath: app.deletingLastPathComponent().path)
            && fm.isWritableFile(atPath: app.path)
    }

    /// At launch (`interactive` false) a failure stays quiet and only a newer release opens the sheet.
    func check(interactive: Bool) async {
        guard enabled else { return }
        switch state {
        case .checking, .downloading, .installing: return
        default: break
        }
        state = .checking
        do {
            var request = URLRequest(url: Self.latestURL)
            request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            let (data, response) = try await URLSession.shared.data(for: request)
            if let http = response as? HTTPURLResponse, http.statusCode != 200 {
                throw UpdateError("HTTP \(http.statusCode)")
            }
            let release = try Self.decode(data)
            if Self.isNewer(release.version, than: Self.currentVersion) {
                state = .available(release)
                if !interactive { prompt = release }
            } else {
                state = interactive ? .upToDate : .idle
            }
        } catch {
            state = interactive ? .failed(L("ui.updater.check_failed", ["detail": error.localizedDescription])) : .idle
        }
    }

    static func decode(_ data: Data) throws -> Release {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(Release.self, from: data)
    }

    /// Compares dotted versions number by number ("0.10.0" > "0.9.1"); a leading "v" is ignored.
    nonisolated static func isNewer(_ candidate: String, than current: String) -> Bool {
        func parts(_ version: String) -> [Int] {
            let trimmed = version.hasPrefix("v") ? version.dropFirst() : Substring(version)
            // "0.2.0-beta" compares as 0.2.0.
            let core = trimmed.prefix { $0 != "-" && $0 != "+" }
            return core.split(separator: ".").map { Int($0) ?? 0 }
        }
        let a = parts(candidate), b = parts(current)
        for i in 0..<max(a.count, b.count) {
            let x = i < a.count ? a[i] : 0, y = i < b.count ? b[i] : 0
            if x != y { return x > y }
        }
        return false
    }

    /// Downloads, verifies and swaps in the release, then relaunches; where it can't, opens the release page.
    func install(_ release: Release) async {
        guard canInstall else {
            NSWorkspace.shared.open(release.htmlUrl)
            return
        }
        state = .downloading(0)
        do {
            guard let asset = release.archive else { throw UpdateError(L("ui.updater.no_archive")) }
            let app = Bundle.main.bundleURL
            // On the app's volume, so the final swap is a rename.
            let work = try FileManager.default.url(
                for: .itemReplacementDirectory, in: .userDomainMask, appropriateFor: app, create: true
            )
            defer { try? FileManager.default.removeItem(at: work) }
            let zip = try await Downloader.download(asset.browserDownloadUrl, to: work.appendingPathComponent(asset.name)) { progress in
                Task { @MainActor in
                    if case .downloading = self.state { self.state = .downloading(progress) }
                }
            }
            state = .installing
            try Self.verify(zip, digest: asset.digest)
            let extracted = work.appendingPathComponent("extracted")
            try await Self.run("/usr/bin/ditto", ["-x", "-k", zip.path, extracted.path])
            let newApp = extracted.appendingPathComponent("Oxide.app")
            let info = Bundle(url: newApp)?.infoDictionary
            guard info?["CFBundleIdentifier"] as? String == Self.bundleIdentifier,
                  info?["CFBundleShortVersionString"] as? String == release.version
            else { throw UpdateError(L("ui.updater.invalid_archive")) }
            try await Self.run("/usr/bin/codesign", ["--verify", "--deep", "--strict", newApp.path])
            _ = try FileManager.default.replaceItemAt(app, withItemAt: newApp)
            relaunch(app)
        } catch {
            state = .failed(L("ui.updater.failed", ["detail": error.localizedDescription]))
        }
    }

    private static func verify(_ file: URL, digest: String?) throws {
        guard let expected = digest?.split(separator: ":").last.map(String.init)?.lowercased() else {
            throw UpdateError(L("ui.updater.checksum_mismatch"))
        }
        let data = try Data(contentsOf: file, options: .mappedIfSafe)
        let actual = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        guard actual == expected else { throw UpdateError(L("ui.updater.checksum_mismatch")) }
    }

    /// Runs a tool off the main thread; a non-zero exit is an error carrying its stderr.
    private static func run(_ tool: String, _ arguments: [String]) async throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        let errors = Pipe()
        process.standardError = errors
        process.standardOutput = FileHandle.nullDevice
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            process.terminationHandler = { process in
                if process.terminationStatus == 0 {
                    continuation.resume()
                } else {
                    let message = String(data: errors.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
                    let name = URL(fileURLWithPath: tool).lastPathComponent
                    continuation.resume(throwing: UpdateError("\(name): \(message.trimmingCharacters(in: .whitespacesAndNewlines))"))
                }
            }
            do { try process.run() } catch { continuation.resume(throwing: error) }
        }
    }

    /// Opens the new bundle once this process has exited; quitting hangs up a call first.
    private func relaunch(_ app: URL) {
        let script = "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; /usr/bin/open \"$2\""
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, "sh", "\(ProcessInfo.processInfo.processIdentifier)", app.path]
        do {
            try process.run()
        } catch {
            state = .failed(L("ui.updater.failed", ["detail": error.localizedDescription]))
            return
        }
        // AppKit cancels quitting while a window has a sheet: close the update sheet first.
        prompt = nil
        Task { @MainActor in
            let deadline = Date().addingTimeInterval(2)
            while Date() < deadline, NSApp.windows.contains(where: { $0.attachedSheet != nil }) {
                try? await Task.sleep(for: .milliseconds(100))
            }
            NSApp.terminate(nil)
        }
    }
}

struct UpdateError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

/// A file download reporting its progress (0…1), which the async URLSession API doesn't.
private final class Downloader: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    private let destination: URL
    private let progress: (Double) -> Void
    private var continuation: CheckedContinuation<URL, Error>?

    private init(destination: URL, progress: @escaping (Double) -> Void) {
        self.destination = destination
        self.progress = progress
    }

    static func download(_ url: URL, to destination: URL, progress: @escaping (Double) -> Void) async throws -> URL {
        let delegate = Downloader(destination: destination, progress: progress)
        let session = URLSession(configuration: .ephemeral, delegate: delegate, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        return try await withCheckedThrowingContinuation { continuation in
            delegate.continuation = continuation
            session.downloadTask(with: url).resume()
        }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData _: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard totalBytesExpectedToWrite > 0 else { return }
        progress(Double(totalBytesWritten) / Double(totalBytesExpectedToWrite))
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        // The temporary file is deleted when this returns: move it now.
        do {
            if let http = downloadTask.response as? HTTPURLResponse, http.statusCode != 200 {
                throw UpdateError("HTTP \(http.statusCode)")
            }
            try FileManager.default.moveItem(at: location, to: destination)
            continuation?.resume(returning: destination)
        } catch {
            continuation?.resume(throwing: error)
        }
        continuation = nil
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let error else { return }
        continuation?.resume(throwing: error)
        continuation = nil
    }
}

/// The update sheet: the new version's notes, then install now or later.
struct UpdatePromptView: View {
    @Environment(\.dismiss) private var dismiss
    let release: Release
    private var updater: Updater { .shared }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                OxideMark().frame(width: 36, height: 36)
                VStack(alignment: .leading, spacing: 3) {
                    Text(L("ui.updater.available_title", ["version": release.version])).font(.figtree(17, .semibold))
                    Muted(L("ui.updater.current_version", ["version": Updater.currentVersion]))
                }
            }
            if !(release.body ?? "").isEmpty {
                Text(L("ui.updater.release_notes")).font(.figtree(13, .semibold))
                ScrollView {
                    Text(release.notes)
                        .font(.figtree(12))
                        .foregroundStyle(Theme.textSoft)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                }
                .frame(maxHeight: 220)
                .card(radius: 8)
            }
            status
            HStack {
                if !updater.canInstall {
                    Muted(L("ui.updater.manual_install"), size: 11).fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                Button(L("ui.updater.later")) { dismiss() }
                    .buttonStyle(OutlineButtonStyle())
                    .keyboardShortcut(.cancelAction)
                    .disabled(busy)
                Button(updater.canInstall ? L("ui.updater.install") : L("ui.updater.open_release_page")) {
                    Task { await updater.install(release) }
                }
                .buttonStyle(PrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(busy)
            }
        }
        .padding(24)
        .frame(width: 480)
        .interactiveDismissDisabled(busy)
    }

    private var busy: Bool {
        switch updater.state {
        case .downloading, .installing: true
        default: false
        }
    }

    @ViewBuilder private var status: some View {
        switch updater.state {
        case .downloading(let fraction):
            ProgressView(value: fraction) { Muted(L("ui.updater.downloading"), size: 11) }
        case .installing:
            ProgressView { Muted(L("ui.updater.installing"), size: 11) }.progressViewStyle(.linear)
        case .failed(let message):
            Text(message).font(.figtree(12)).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
        default:
            EmptyView()
        }
    }
}

/// Settings' footer line: check now, then the outcome or the newer version to install.
struct UpdateSettingsRow: View {
    private var updater: Updater { .shared }
    var body: some View {
        if updater.enabled {
            VStack(spacing: 6) {
                switch updater.state {
                case .available(let release):
                    Button(L("ui.updater.install_version", ["version": release.version])) { updater.prompt = release }
                        .buttonStyle(PrimaryButtonStyle())
                default:
                    Button(L("ui.updater.check")) { Task { await updater.check(interactive: true) } }
                        .buttonStyle(OutlineButtonStyle())
                        .disabled(updater.state == .checking)
                }
                switch updater.state {
                case .checking: Muted(L("ui.updater.checking"), size: 11)
                case .upToDate: Muted(L("ui.updater.up_to_date"), size: 11)
                case .failed(let message): Muted(message, size: 11).multilineTextAlignment(.center)
                default: EmptyView()
                }
            }
        }
    }
}
