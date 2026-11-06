import AppKit
import OxideCore

extension NSAttributedString.Key {
    static let composerBlock = NSAttributedString.Key("OxideBlock")
    static let composerIndent = NSAttributedString.Key("OxideIndent")
    static let composerCode = NSAttributedString.Key("OxideCode")
}

enum ComposerText {
    static let blocks: [ComposerBlock] = [.paragraph, .heading1, .heading2, .heading3, .bullet, .numbered, .quote, .codeBlock, .rule]
    static var baseFont: NSFont { NSFont(name: "Figtree-Regular", size: 14) ?? .systemFont(ofSize: 14) }
    static var base: [NSAttributedString.Key: Any] { [.font: baseFont, .foregroundColor: NSColor.labelColor] }
    static var empty: ComposerFormat { format(base, range: NSRange(location: 0, length: 0)) }
    static func hex(_ color: NSColor?) -> String? {
        guard let color, color != .labelColor, let rgb = color.usingColorSpace(.sRGB) else { return nil }
        return String(format: "#%02x%02x%02x", Int((rgb.redComponent * 255).rounded()), Int((rgb.greenComponent * 255).rounded()), Int((rgb.blueComponent * 255).rounded()))
    }
    static func color(_ hex: String?) -> NSColor? {
        guard let hex else { return nil }
        let digits = hex.dropFirst()
        let expanded = digits.count == 3 ? digits.map { "\($0)\($0)" }.joined() : String(digits)
        guard hex.first == "#", expanded.count == 6, let rgb = UInt32(expanded, radix: 16) else { return nil }
        return NSColor(srgbRed: CGFloat((rgb >> 16) & 255) / 255, green: CGFloat((rgb >> 8) & 255) / 255, blue: CGFloat(rgb & 255) / 255, alpha: 1)
    }
    static func block(_ attrs: [NSAttributedString.Key: Any]) -> ComposerBlock {
        let index = (attrs[.composerBlock] as? NSNumber)?.intValue ?? 0
        return blocks.indices.contains(index) ? blocks[index] : .paragraph
    }
    static func format(_ attrs: [NSAttributedString.Key: Any], range: NSRange) -> ComposerFormat {
        let font = attrs[.font] as? NSFont ?? baseFont
        let traits = NSFontManager.shared.traits(of: font)
        let paragraph = attrs[.paragraphStyle] as? NSParagraphStyle
        return ComposerFormat(start: UInt32(range.location), length: UInt32(range.length),
            bold: traits.contains(.boldFontMask), italic: traits.contains(.italicFontMask),
            underline: (attrs[.underlineStyle] as? NSNumber)?.intValue ?? 0 != 0,
            strike: (attrs[.strikethroughStyle] as? NSNumber)?.intValue ?? 0 != 0,
            code: (attrs[.composerCode] as? Bool) == true,
            color: hex(attrs[.foregroundColor] as? NSColor), highlight: hex(attrs[.backgroundColor] as? NSColor),
            size: font.pointSize == 14 ? 0 : UInt32(min(max(font.pointSize, 10), 36)),
            link: (attrs[.link] as? URL)?.absoluteString ?? attrs[.link] as? String,
            block: block(attrs), indent: UInt32((attrs[.composerIndent] as? NSNumber)?.intValue ?? 0),
            alignment: paragraph?.alignment == .center ? "center" : paragraph?.alignment == .right ? "right" : "left",
            image: (attrs[.attachment] as? PastedImageAttachment)?.id)
    }
    static func formats(_ text: NSTextView) -> [ComposerFormat] {
        guard let storage = text.textStorage else { return [] }
        var result: [ComposerFormat] = []
        storage.enumerateAttributes(in: NSRange(location: 0, length: storage.length)) { attrs, range, _ in
            result.append(format(attrs, range: range))
        }
        return result
    }
    static func attributes(_ style: ComposerFormat) -> [NSAttributedString.Key: Any] {
        var attrs = base
        var font = style.code || style.block == .codeBlock ? NSFont.monospacedSystemFont(ofSize: 14, weight: .regular) : baseFont
        font = NSFontManager.shared.convert(font, toSize: style.size == 0 ? 14 : CGFloat(style.size))
        if style.bold { font = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask) }
        if style.italic { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
        attrs[.font] = font
        if style.underline { attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue }
        if style.strike { attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
        if let color = color(style.color) { attrs[.foregroundColor] = color }
        if let color = color(style.highlight) { attrs[.backgroundColor] = color }
        if let link = style.link { attrs[.link] = link }
        if let id = style.image, let image = PastedImageAttachment.images[id] {
            attrs[.attachment] = PastedImageAttachment(id: id, image: image)
        }
        attrs[.composerCode] = style.code
        attrs[.composerBlock] = blocks.firstIndex(of: style.block) ?? 0
        attrs[.composerIndent] = style.indent
        let paragraph = NSMutableParagraphStyle()
        let extra: CGFloat = [.bullet, .numbered, .quote].contains(style.block) ? 24 : 0
        paragraph.headIndent = CGFloat(style.indent) * 24 + extra
        paragraph.firstLineHeadIndent = paragraph.headIndent
        paragraph.paragraphSpacing = 4
        paragraph.alignment = style.alignment == "center" ? .center : style.alignment == "right" ? .right : .left
        attrs[.paragraphStyle] = paragraph
        return attrs
    }
    /// The format text typed at `caret` takes, from the run the core would extend there; like AppKit, neither an
    /// image nor a link carries on into typed text.
    static func typing(_ formats: [ComposerFormat], at caret: Int) -> ComposerFormat {
        var style = formats.first { Int($0.start) <= caret && caret <= Int($0.start + $0.length) } ?? empty
        style.image = nil
        style.link = nil
        return style
    }
    /// Space above and below the text in the editor.
    static let verticalInset: CGFloat = 4
    static let lineFragmentPadding: CGFloat = 2
    /// The editor's height for the text laid out in `container`.
    static func height(of layout: NSLayoutManager, in container: NSTextContainer) -> CGFloat {
        layout.ensureLayout(for: container)
        return layout.usedRect(for: container).height + verticalInset * 2
    }
    static func attributed(_ string: String, formats: [ComposerFormat]) -> NSAttributedString {
        let result = NSMutableAttributedString(string: string, attributes: base)
        for style in formats {
            let range = NSRange(location: Int(style.start), length: Int(style.length))
            if NSMaxRange(range) <= result.length { result.setAttributes(attributes(style), range: range) }
        }
        return result
    }
}

/// A pasted image in the draft, shown at most 240 by 120 points; `id` is what the core knows its upload by.
final class PastedImageAttachment: NSTextAttachment {
    let id: String
    /// Pasted images by id, to show them again when the draft is rebuilt, e.g. back from another conversation.
    static var images: [String: NSImage] = [:]

    init(id: String, image: NSImage) {
        self.id = id
        super.init(data: nil, ofType: nil)
        self.image = image
        let size = image.size
        let scale = min(1, 240 / max(size.width, 1), 120 / max(size.height, 1))
        bounds = CGRect(x: 0, y: 0, width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
    }
    required init?(coder: NSCoder) { nil }
}

/// List markers are drawn separately: the draft retains clean text and mention offsets.
final class ComposerLayoutManager: NSLayoutManager {
    override func drawGlyphs(forGlyphRange glyphsToShow: NSRange, at origin: NSPoint) {
        super.drawGlyphs(forGlyphRange: glyphsToShow, at: origin)
        guard let storage = textStorage, storage.length > 0 else { return }
        let characters = characterRange(forGlyphRange: glyphsToShow, actualGlyphRange: nil)
        let string = storage.string as NSString
        var start = string.paragraphRange(for: NSRange(location: characters.location, length: 0)).location
        var number = 0
        // Include preceding paragraphs to keep ordered-list numbers stable while scrolling.
        var scan = 0
        while scan < start {
            let range = string.paragraphRange(for: NSRange(location: scan, length: 0))
            number = ComposerText.block(storage.attributes(at: scan, effectiveRange: nil)) == .numbered ? number + 1 : 0
            scan = NSMaxRange(range)
        }
        while start < min(NSMaxRange(characters), storage.length) {
            let range = string.paragraphRange(for: NSRange(location: start, length: 0))
            let attrs = storage.attributes(at: start, effectiveRange: nil)
            let block = ComposerText.block(attrs)
            number = block == .numbered ? number + 1 : 0
            if [.bullet, .numbered, .quote].contains(block) {
                let glyph = glyphIndexForCharacter(at: start)
                let rect = lineFragmentUsedRect(forGlyphAt: glyph, effectiveRange: nil)
                let indent = (attrs[.composerIndent] as? NSNumber)?.intValue ?? 0
                let marker = block == .bullet ? "•" : block == .numbered ? "\(number)." : "│"
                (marker as NSString).draw(at: NSPoint(x: origin.x + CGFloat(indent * 24) + 2, y: origin.y + rect.minY), withAttributes: [.font: ComposerText.baseFont, .foregroundColor: NSColor.secondaryLabelColor])
            }
            start = NSMaxRange(range)
        }
    }
}

final class ComposerTextView: NSTextView {
    static let automaticSpellingCorrectionKey = "composer.automaticSpellingCorrection"

    var formatShortcut: ((String) -> Void)?
    var sendShortcut: (() -> Void)?
    /// Receives an image pasted on its own, as PNG with its size in points, before it goes into the text under `id`.
    var imagePaste: ((_ id: String, _ png: Data, _ size: NSSize) -> Void)?
    /// Receives the paths of files dropped from the Finder, which the text view would otherwise write out as text.
    var fileDrop: (([String]) -> Void)?
    var preferences = UserDefaults.standard
    var pasteboard = NSPasteboard.general

    /// Applies the choice last made from the Spelling menu: each conversation gets a new composer, which would
    /// otherwise follow the system setting.
    func restoreAutomaticSpellingCorrection() {
        guard let enabled = preferences.object(forKey: Self.automaticSpellingCorrectionKey) as? Bool else { return }
        isAutomaticSpellingCorrectionEnabled = enabled
    }

    override func toggleAutomaticSpellingCorrection(_ sender: Any?) {
        super.toggleAutomaticSpellingCorrection(sender)
        preferences.set(isAutomaticSpellingCorrectionEnabled, forKey: Self.automaticSpellingCorrectionKey)
    }
    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        let paths = Self.droppedFiles(from: sender.draggingPasteboard)
        guard let fileDrop, !paths.isEmpty else { return super.performDragOperation(sender) }
        fileDrop(paths)
        return true
    }
    /// Local file paths on a dragging pasteboard; dragged text and web links carry none.
    static func droppedFiles(from pasteboard: NSPasteboard) -> [String] {
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
        return (urls ?? []).map(\.path)
    }
    /// Shows the draft the core holds, with the caret at `cursor`, typing on in the core's format there.
    func showDraft(_ draft: String, formats: [ComposerFormat], cursor: Int) {
        let caret = min(cursor, (draft as NSString).length)
        textStorage?.setAttributedString(ComposerText.attributed(draft, formats: formats))
        setSelectedRange(NSRange(location: caret, length: 0))
        typingAttributes = ComposerText.attributes(ComposerText.typing(formats, at: caret))
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.contains(.command), [36, 76].contains(event.keyCode), !hasMarkedText() {
            sendShortcut?()
            return true
        }
        if event.modifierFlags.contains(.command), let key = event.charactersIgnoringModifiers?.lowercased() {
            let command = key == "x" && event.modifierFlags.contains(.shift) ? "strike" : key
            if ["b", "i", "u", "k", "strike"].contains(command) { formatShortcut?(command); return true }
        }
        return super.performKeyEquivalent(with: event)
    }
    /// The caret spans its line, as tall as an image on it: it keeps to the text's height, at the bottom of the line.
    override func drawInsertionPoint(in rect: NSRect, color: NSColor, turnedOn flag: Bool) {
        super.drawInsertionPoint(in: Self.caret(in: rect, font: typingAttributes[.font] as? NSFont ?? ComposerText.baseFont,
                                                layout: layoutManager), color: color, turnedOn: flag)
    }
    static func caret(in rect: NSRect, font: NSFont, layout: NSLayoutManager?) -> NSRect {
        let height = min(rect.height, layout?.defaultLineHeight(for: font) ?? rect.height)
        return NSRect(x: rect.minX, y: rect.maxY - height, width: rect.width, height: height)
    }
    override func paste(_ sender: Any?) {
        if let imagePaste, let png = Self.pastedImage(from: pasteboard), let image = NSImage(data: png) {
            let id = UUID().uuidString
            PastedImageAttachment.images[id] = image
            imagePaste(id, png, image.size)
            insertText(NSAttributedString(attachment: PastedImageAttachment(id: id, image: image)), replacementRange: selectedRange())
            return
        }
        super.paste(sender)
    }

    /// The text view greys out Paste for content it cannot read, and it reads no images (`importsGraphics` is off).
    override func validateUserInterfaceItem(_ item: any NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(paste(_:)), imagePaste != nil, Self.holdsImageAlone(pasteboard) { return true }
        return super.validateUserInterfaceItem(item)
    }

    /// An image (a screenshot, a copied picture) with no text or file to paste instead: text copied from a document,
    /// which may carry its pictures too, still pastes as text.
    static func holdsImageAlone(_ pasteboard: NSPasteboard) -> Bool {
        let types = pasteboard.types ?? []
        return !types.contains(.string) && !types.contains(.fileURL) && NSImage.canInit(with: pasteboard)
    }

    /// The PNG of an image pasted on its own, see `holdsImageAlone`.
    static func pastedImage(from pasteboard: NSPasteboard) -> Data? {
        guard holdsImageAlone(pasteboard) else { return nil }
        if let png = pasteboard.data(forType: .png) { return png }
        guard let image = NSImage(pasteboard: pasteboard), let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff) else { return nil }
        return bitmap.representation(using: .png, properties: [:])
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        addCursorRect(bounds, cursor: .iBeam)
    }
    override func cursorUpdate(with event: NSEvent) { NSCursor.iBeam.set() }
}
