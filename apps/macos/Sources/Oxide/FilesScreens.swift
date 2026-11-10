import AppKit
import Quartz
import SwiftUI
import OxideCore
import UniformTypeIdentifiers

/// Native panels around the core's file actions: it never shows a dialog itself.
@MainActor
enum FilePanels {
    static func chooseFiles(prompt: String) -> [String] {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.prompt = prompt
        return panel.runModal() == .OK ? panel.urls.map(\.path) : []
    }
    static func saveLocation(for name: String) -> String? {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = name
        panel.directoryURL = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
        panel.canCreateDirectories = true
        return panel.runModal() == .OK ? panel.url?.path : nil
    }
    /// Local file paths dropped from the Finder.
    static func paths(from providers: [NSItemProvider], then deliver: @escaping @MainActor ([String]) -> Void) -> Bool {
        let files = providers.filter { $0.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) }
        guard !files.isEmpty else { return false }
        let group = DispatchGroup()
        let collected = Collected()
        for provider in files {
            group.enter()
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                if let url, url.isFileURL { collected.add(url.path) }
                group.leave()
            }
        }
        group.notify(queue: .main) {
            MainActor.assumeIsolated { deliver(collected.paths) }
        }
        return true
    }
    private final class Collected: @unchecked Sendable {
        private let lock = NSLock()
        private var values: [String] = []
        func add(_ path: String) { lock.lock(); values.append(path); lock.unlock() }
        var paths: [String] { lock.lock(); defer { lock.unlock() }; return values }
    }
    /// Downloads to a location the user picks.
    static func download(_ store: Store, key: String, name: String) {
        if let path = saveLocation(for: name) {
            store.send(.files(action: .download(key: key, path: path)))
        }
    }
}

/// Shows one downloaded file in the Quick Look panel.
@MainActor
final class QuickLook: NSObject, QLPreviewPanelDataSource {
    static let shared = QuickLook()
    private var url: URL?

    func show(_ path: String) {
        url = URL(fileURLWithPath: path)
        guard let panel = QLPreviewPanel.shared() else { return }
        panel.dataSource = self
        panel.reloadData()
        panel.makeKeyAndOrderFront(nil)
    }
    nonisolated func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int {
        MainActor.assumeIsolated { url == nil ? 0 : 1 }
    }
    nonisolated func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! {
        MainActor.assumeIsolated { url as NSURL? }
    }
}

/// OneDrive's folder colours, indexed by `_ColorHex`; 0 keeps the app's own look.
enum FolderColors {
    static let count: UInt8 = 16
    private static let hex: [UInt32] = [
        0x000000, 0xA4262C, 0xCA5010, 0x0B6A0B, 0x038387, 0x004E8C, 0x5C2E91, 0xBF0077,
        0x7A7574, 0xF1707B, 0xFFAA44, 0x6BB700, 0x00B7C3, 0x73B3EB, 0x8764B8, 0xEE5FB7,
    ]
    static func color(_ index: UInt8) -> Color {
        index == 0 || index >= count ? Theme.accentText : Theme.rgb(hex[Int(index)])
    }
    static func name(_ index: UInt8) -> String { L("ui.files_screens.folder_color_\(index)") }
    /// Menus draw SF Symbols in one colour, so the swatch is a plain image.
    static func swatch(_ index: UInt8) -> Image {
        let fill = NSColor(color(index))
        let image = NSImage(size: NSSize(width: 12, height: 12), flipped: false) { rect in
            fill.setFill()
            NSBezierPath(ovalIn: rect.insetBy(dx: 0.5, dy: 0.5)).fill()
            return true
        }
        image.isTemplate = false
        return Image(nsImage: image)
    }
}

/// Extension badge, PDF in its own colors like Teams.
struct FileBadge: View {
    let ext: String
    var folder = false
    var folderColor: UInt8 = 0
    var side: CGFloat = 36
    var body: some View {
        Group {
            if folder {
                Image(systemName: "folder.fill").font(.system(size: side * 0.5))
            } else if ext == "URL" {
                Image(systemName: "link").font(.system(size: side * 0.42, weight: .semibold))
            } else {
                Text(ext).font(.figtree(side * 0.28, .semibold))
            }
        }
        .foregroundStyle(ext == "PDF" ? Theme.pdfText : tinted ? FolderColors.color(folderColor) : Theme.accentText)
        .frame(width: side, height: side)
        .background(
            ext == "PDF" ? Theme.pdfBackground : tinted ? FolderColors.color(folderColor).opacity(0.16) : Theme.accentSoft,
            in: RoundedRectangle(cornerRadius: side * 0.22)
        )
        .accessibilityHidden(true)
    }
    private var tinted: Bool { folder && folderColor != 0 }
}

/// A file attached to a message, or listed in a "Partagé" tab.
struct FileCard: View {
    @Environment(Store.self) private var store
    let file: FileView
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let thumbnail = file.thumbnail, case .ready = thumbnail.status {
                PreviewImage(image: thumbnail, height: 150, savable: false)
                    .padding([.horizontal, .top], 8)
            }
            HStack(spacing: 12) {
                FileBadge(ext: file.ext)
                VStack(alignment: .leading, spacing: 2) {
                    Text(file.name).font(.figtree(13, .semibold)).lineLimit(1)
                    HStack(spacing: 6) {
                        Muted(file.description).lineLimit(1)
                        if !file.date.isEmpty { Muted("· \(file.date)") }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if let progress = file.progress {
                    ProgressView(value: Double(progress), total: 100)
                        .frame(width: 60)
                        .help(L("ui.files_screens.downloading", ["progress": progress]))
                    IconButton(symbol: "xmark.circle", help: L("ui.files_screens.cancel_download"), side: 28, glyph: 13) {
                        store.send(.files(action: .cancel(key: file.key)))
                    }
                } else {
                    Button(L("ui.files_screens.open")) {
                        store.send(.files(action: .open(key: file.key)))
                    }
                    .buttonStyle(OutlineButtonStyle())
                    .disabled(file.url == nil)
                    actions
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
        }
        .card(radius: 10)
        .contextMenu { menu }
    }
    private var actions: some View {
        Menu {
            menu
        } label: {
            Image(systemName: "ellipsis").frame(width: 28, height: 28)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(L("ui.files_screens.more_actions"))
        .accessibilityLabel(L("ui.files_screens.more_actions_for", ["name": file.name]))
        .disabled(file.deleted)
    }
    @ViewBuilder private var menu: some View {
        Button(L("ui.files_screens.open_in_browser")) { store.send(.files(action: .open(key: file.key))) }
            .disabled(file.url == nil)
        if file.canDownload {
            Button(L("ui.files_screens.preview")) { store.send(.files(action: .preview(key: file.key))) }
            Button(L("ui.files_screens.download")) { FilePanels.download(store, key: file.key, name: file.name) }
        }
        Button(L("ui.files_screens.copy_link")) { store.send(.files(action: .copyLink(key: file.key))) }
            .disabled(file.url == nil)
    }
}

/// Files of the draft, above the editor: progress, failure, removal.
struct AttachmentChips: View {
    @Environment(Store.self) private var store
    let attachments: [PendingFileView]
    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(attachments, id: \.id) { file in
                    HStack(spacing: 8) {
                        FileBadge(ext: file.ext, side: 26)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(file.name).font(.figtree(12, .semibold)).lineLimit(1)
                            if let error = file.error {
                                Text(error).font(.figtree(11)).foregroundStyle(Theme.danger).lineLimit(1).help(error)
                            } else if !file.ready {
                                ProgressView(value: Double(file.progress), total: 100).frame(width: 120)
                            } else {
                                Muted(L("ui.files_screens.ready"), size: 11)
                            }
                        }
                        .frame(maxWidth: 180, alignment: .leading)
                        IconButton(symbol: "xmark", help: L("ui.files_screens.remove", ["name": file.name]), side: 22, glyph: 10) {
                            store.send(.files(action: .removeAttachment(id: file.id)))
                        }
                    }
                    .padding(6)
                    .background(Theme.input, in: RoundedRectangle(cornerRadius: 8))
                }
            }
        }
    }
}

extension SharedFilter: Identifiable {
    public var id: Self { self }
    var label: String {
        switch self {
        case .all: L("ui.files_screens.all")
        case .files: L("ui.files_screens.files")
        case .links: L("ui.files_screens.links")
        }
    }
}

/// Search box of the file screens; it empties when the core's query (`current`) ends.
struct FileSearchField: View {
    let placeholder: String
    let current: String
    /// Pause in typing before searching; zero for local filters.
    let delay: Duration
    let search: (String) -> Void
    @State private var query = ""
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(Theme.subtle)
            TextField(placeholder, text: $query)
                .textFieldStyle(.plain)
                .font(.figtree(13))
                .focused($focused)
                .onSubmit { search(query) }
                .onExitCommand { query = "" }
            if !query.isEmpty {
                IconButton(symbol: "xmark.circle.fill", help: L("ui.files_screens.clear_search"), side: 22, glyph: 12) {
                    query = ""
                }
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .frame(maxWidth: 420)
        .background(Theme.input, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(focused ? Theme.accent : .clear))
        .task(id: query) {
            guard query.trimmingCharacters(in: .whitespaces) != current else { return }
            if !query.isEmpty && delay > .zero {
                try? await Task.sleep(for: delay)
                guard !Task.isCancelled else { return }
            }
            search(query)
        }
        .onAppear { query = current }
        .onChange(of: current) { _, value in
            if value.isEmpty, !focused { query = "" }
        }
    }
}

/// "Partagé" tab of a chat: files and links shared in the conversation.
struct SharedScreen: View {
    @Environment(Store.self) private var store
    var body: some View {
        let view = store.files
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                HStack {
                    Text(L("ui.files_screens.shared")).font(.figtree(22, .semibold)).frame(maxWidth: .infinity, alignment: .leading)
                    if view?.loading == true { ProgressView().controlSize(.small) }
                    IconButton(symbol: "arrow.clockwise", help: L("ui.files_screens.refresh")) {
                        store.send(.files(action: .refreshShared))
                    }
                }
                SegmentedPicker(
                    selection: Binding(
                        get: { view?.filter ?? .all },
                        set: { store.send(.files(action: .sharedFilter(filter: $0))) }
                    ),
                    options: [.all, .files, .links],
                    label: \.label
                )
                .fixedSize()
                // The list is complete for the conversation: filtering it is instant.
                FileSearchField(
                    placeholder: L("ui.files_screens.search_for_a_file_link_or"),
                    current: view?.query ?? "",
                    delay: .zero
                ) { store.send(.files(action: .sharedSearch(query: $0))) }
                if let error = view?.error {
                    Muted(L("ui.files_screens.full_list_unavailable_files_from_loaded", ["error": error]))
                }
                let files = view?.files ?? []
                if files.isEmpty && view?.loading != true {
                    let searching = view?.query.isEmpty == false
                    VStack(spacing: 12) {
                        Image(systemName: searching ? "magnifyingglass" : "doc.on.doc").font(.system(size: 38)).foregroundStyle(Theme.subtle)
                        Muted(searching ? L("ui.files_screens.no_results") : L("ui.files_screens.nothing_has_been_shared_in_this"), size: 14)
                    }
                    .padding(40)
                    .frame(maxWidth: .infinity)
                }
                ForEach(files, id: \.key) { FileCard(file: $0) }
            }
            .padding(28)
        }
    }
}

/// OneDrive (global Files) or a channel's SharePoint folder.
struct FileBrowserScreen: View {
    @Environment(Store.self) private var store
    /// Inside a channel: no OneDrive places.
    let embedded: Bool
    @State private var naming: Naming?
    @State private var deleting: EntryView?
    @State private var dropping = false
    /// Row chosen with a click or the arrows: Return opens it, Space previews it.
    @State private var selected: String?
    @FocusState private var listFocused: Bool

    private enum Naming: Identifiable {
        case folder
        case rename(EntryView)
        var id: String {
            switch self {
            case .folder: "new"
            case .rename(let entry): entry.key
            }
        }
    }

    var body: some View {
        HStack(spacing: 0) {
            if !embedded {
                places.frame(width: 200)
                Divider().overlay(Theme.border)
            }
            content
        }
        .sheet(item: $naming) { naming in
            NameSheet(
                title: { if case .folder = naming { L("ui.files_screens.new_folder") } else { L("ui.files_screens.rename") } }(),
                initial: { if case .rename(let e) = naming { e.name } else { "" } }()
            ) { name in
                switch naming {
                case .folder: store.send(.files(action: .newFolder(name: name)))
                case .rename(let entry): store.send(.files(action: .rename(key: entry.key, name: name)))
                }
            }
        }
        .confirmationDialog(
            L("ui.files_screens.delete", ["name": deleting?.name ?? ""]),
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            titleVisibility: .visible
        ) {
            Button(L("ui.files_screens.move_to_recycle_bin"), role: .destructive) {
                if let entry = deleting { store.send(.files(action: .delete(key: entry.key))) }
                deleting = nil
            }
        } message: {
            Text(L("ui.files_screens.the_item_goes_to_the_sharepoint"))
        }
    }

    private var places: some View {
        let current = store.browser?.place
        return VStack(alignment: .leading, spacing: 4) {
            Text(L("ui.files_screens.files")).font(.figtree(20, .semibold)).padding(.bottom, 10)
            place(L("ui.files_screens.recent"), "clock", .recent, current)
            place(L("ui.files_screens.my_files"), "folder", .myFiles, current)
            place(L("ui.files_screens.shared_with_me"), "person.2", .sharedWithMe, current)
            Spacer()
        }
        .padding(16)
    }
    private func place(_ name: String, _ symbol: String, _ value: BrowserPlace, _ current: BrowserPlace?) -> some View {
        Button {
            store.send(.files(action: .browse(place: value)))
        } label: {
            Label(name, systemImage: symbol)
                .font(.figtree(13, current == value ? .semibold : .regular))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 7).padding(.horizontal, 10)
                .background(current == value ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 6))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain).pointerCursor()
        .accessibilityAddTraits(current == value ? .isSelected : [])
    }

    @ViewBuilder private var content: some View {
        let view = store.browser
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                crumbs(view)
                Spacer()
                if view?.loading == true || view?.busy == true { ProgressView().controlSize(.small) }
                if view?.canWrite == true {
                    Button { naming = .folder } label: { Label(L("ui.files_screens.new_folder"), systemImage: "folder.badge.plus") }
                        .buttonStyle(OutlineButtonStyle())
                    Button { upload(FilePanels.chooseFiles(prompt: L("ui.files_screens.upload"))) } label: { Label(L("ui.files_screens.upload"), systemImage: "arrow.up.doc") }
                        .buttonStyle(OutlineButtonStyle())
                }
                IconButton(symbol: "arrow.clockwise", help: L("ui.files_screens.refresh")) {
                    store.send(.files(action: .refreshBrowser))
                }
            }
            searchField(view)
            if let error = view?.error {
                Text(error).font(.figtree(12)).foregroundStyle(Theme.danger)
            }
            table(view?.entries ?? [], loading: view?.loading == true)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .overlay {
            if dropping {
                RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.accent, style: StrokeStyle(lineWidth: 2, dash: [6]))
                    .padding(8)
            }
        }
        .onDrop(of: [.fileURL], isTargeted: $dropping) { providers in
            guard view?.canWrite == true else { return false }
            return FilePanels.paths(from: providers, then: upload)
        }
    }

    private func upload(_ paths: [String]) {
        if !paths.isEmpty { store.send(.files(action: .uploadHere(paths: paths))) }
    }

    /// Searches the place shown: the whole OneDrive, or the channel's folder.
    private func searchField(_ view: BrowserView?) -> some View {
        FileSearchField(
            placeholder: view?.place == .channel ? L("ui.files_screens.search_channel_files") : L("ui.files_screens.search_my_files"),
            current: view?.query ?? "",
            delay: .milliseconds(350)
        ) { store.send(.files(action: .searchFiles(query: $0))) }
    }

    @ViewBuilder private func crumbs(_ view: BrowserView?) -> some View {
        let crumbs = view?.crumbs ?? []
        if crumbs.isEmpty {
            Text(view?.title ?? L("ui.files_screens.files")).font(.figtree(20, .semibold))
        } else {
            HStack(spacing: 4) {
                ForEach(Array(crumbs.enumerated()), id: \.offset) { index, name in
                    if index > 0 { Image(systemName: "chevron.right").font(.system(size: 11)).foregroundStyle(Theme.subtle) }
                    let last = index == crumbs.count - 1
                    Button(name) { store.send(.files(action: .crumb(index: UInt32(index)))) }
                        .buttonStyle(.plain)
                        .font(.figtree(last ? 20 : 15, last ? .semibold : .regular))
                        .foregroundStyle(last ? Theme.text : Theme.muted)
                        .disabled(last)
                        .pointerCursor()
                }
            }
            .lineLimit(1)
        }
    }

    @ViewBuilder private func table(_ entries: [EntryView], loading: Bool) -> some View {
        if entries.isEmpty && !loading {
            VStack(spacing: 12) {
                Image(systemName: "folder").font(.system(size: 38)).foregroundStyle(Theme.subtle)
                Muted(store.browser?.query.isEmpty == false ? L("ui.files_screens.no_results") : L("ui.files_screens.this_folder_is_empty"), size: 14)
            }
            .padding(40)
            .frame(maxWidth: .infinity)
        } else {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        HStack {
                            Text(L("ui.files_screens.name")).frame(maxWidth: .infinity, alignment: .leading)
                            Text(L("ui.files_screens.modified")).frame(width: 90, alignment: .leading)
                            Text(L("ui.files_screens.modified_by")).frame(width: 140, alignment: .leading)
                            Text(L("ui.files_screens.size")).frame(width: 90, alignment: .trailing)
                            Color.clear.frame(width: 30)
                        }
                        .font(.figtree(11, .semibold))
                        .foregroundStyle(Theme.muted)
                        .padding(.horizontal, 10).padding(.bottom, 6)
                        Divider().overlay(Theme.border)
                        ForEach(entries, id: \.key) { entry in
                            row(entry).id(entry.key)
                            Divider().overlay(Theme.border)
                        }
                    }
                }
                .focusable()
                .focused($listFocused)
                .focusEffectDisabled()
                .onKeyPress(.upArrow) { move(-1, in: entries, proxy: proxy) }
                .onKeyPress(.downArrow) { move(1, in: entries, proxy: proxy) }
                .onKeyPress(.return) {
                    guard let entry = entries.first(where: { $0.key == selected }) else { return .ignored }
                    open(entry)
                    return .handled
                }
                .onKeyPress(.space) {
                    guard let entry = entries.first(where: { $0.key == selected }), !entry.folder else { return .ignored }
                    store.send(.files(action: .preview(key: entry.key)))
                    return .handled
                }
                .accessibilityLabel(L("ui.files_screens.files"))
                .accessibilityHint(L("ui.files_screens.arrows_to_select_return_to_open"))
            }
        }
    }

    private func move(_ offset: Int, in entries: [EntryView], proxy: ScrollViewProxy) -> KeyPress.Result {
        guard !entries.isEmpty else { return .ignored }
        let index = entries.firstIndex(where: { $0.key == selected }).map { $0 + offset } ?? (offset > 0 ? 0 : entries.count - 1)
        let key = entries[min(max(index, 0), entries.count - 1)].key
        selected = key
        proxy.scrollTo(key)
        return .handled
    }

    private func row(_ entry: EntryView) -> some View {
        HStack(spacing: 10) {
            FileBadge(ext: entry.ext, folder: entry.folder, folderColor: entry.color, side: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(entry.name).font(.figtree(13)).lineLimit(1)
                if !entry.location.isEmpty {
                    Label(entry.location, systemImage: "folder").font(.figtree(11)).foregroundStyle(Theme.muted).lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if let progress = entry.progress {
                ProgressView(value: Double(progress), total: 100).frame(width: 80)
            }
            Text(entry.modified).frame(width: 90, alignment: .leading)
            Text(entry.modifiedBy).lineLimit(1).frame(width: 140, alignment: .leading)
            Text(entry.size).frame(width: 90, alignment: .trailing)
            Menu { menu(entry) } label: { Image(systemName: "ellipsis").frame(width: 24, height: 24) }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .accessibilityLabel(L("ui.files_screens.actions_for", ["name": entry.name]))
        }
        .font(.figtree(12))
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background(selected == entry.key ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle())
        .onTapGesture(count: 2) { open(entry) }
        .simultaneousGesture(TapGesture().onEnded {
            selected = entry.key
            listFocused = true
        })
        .contextMenu { menu(entry) }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected == entry.key ? .isSelected : [])
        .accessibilityAction(named: entry.folder ? L("ui.files_screens.open_folder") : L("ui.files_screens.open")) { open(entry) }
    }

    private func open(_ entry: EntryView) {
        if entry.canOpenFolder {
            store.send(.files(action: .openFolder(key: entry.key)))
        } else {
            store.send(.files(action: .open(key: entry.key)))
        }
    }

    @ViewBuilder private func menu(_ entry: EntryView) -> some View {
        Button(entry.folder ? L("ui.files_screens.open") : L("ui.files_screens.open_in_browser")) { open(entry) }
        if !entry.folder {
            Button(L("ui.files_screens.preview")) { store.send(.files(action: .preview(key: entry.key))) }
            Button(L("ui.files_screens.download")) { FilePanels.download(store, key: entry.key, name: entry.name) }
        }
        Button(L("ui.files_screens.copy_link")) { store.send(.files(action: .copyLink(key: entry.key))) }
        if store.browser?.canWrite == true {
            Divider()
            Button(L("ui.files_screens.rename_2")) { naming = .rename(entry) }
            if entry.folder {
                Picker(L("ui.files_screens.folder_color"), selection: Binding(
                    get: { entry.color },
                    set: { store.send(.files(action: .setColor(key: entry.key, color: $0))) }
                )) {
                    ForEach(0..<FolderColors.count, id: \.self) { index in
                        Label { Text(FolderColors.name(index)) } icon: { FolderColors.swatch(index) }.tag(index)
                    }
                }
            }
            Button(L("ui.files_screens.delete_2"), role: .destructive) { deleting = entry }
        }
    }
}

/// Asks for a folder or file name.
private struct NameSheet: View {
    @Environment(\.dismiss) private var dismiss
    let title: String
    let initial: String
    let done: (String) -> Void
    @State private var name = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(title).font(.figtree(16, .semibold))
            TextField(L("ui.files_screens.name"), text: $name)
                .textFieldStyle(.roundedBorder)
                .onSubmit(submit)
            HStack {
                Spacer()
                Button(L("ui.files_screens.cancel")) { dismiss() }.keyboardShortcut(.cancelAction)
                Button(L("ui.files_screens.ok"), action: submit)
                    .keyboardShortcut(.defaultAction)
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 360)
        .onAppear { name = initial }
    }
    private func submit() {
        let value = name.trimmingCharacters(in: .whitespaces)
        guard !value.isEmpty else { return }
        done(value)
        dismiss()
    }
}
