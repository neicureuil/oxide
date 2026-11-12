import AppKit
import SwiftUI
import OxideCore

/// A message body as one AppKit text wherever its blocks are text-like, so a selection runs across paragraphs, lists,
/// quotes and tables as it does in Teams' web view. Images, cards and code stay views of their own, cutting the text
/// where they sit, even inside a list or a quote; the feed's selection runs across them.
struct MessageBody: View {
    @Environment(\.openURL) private var openURL
    @Environment(\.cardMessageId) private var messageId
    let blocks: [BlockView]
    var size: Double = 14
    /// Whether the texts belong to the message's selection across the feed; a card's texts select on their own.
    var inFeedSelection = true

    var body: some View {
        // Read while building, so SwiftUI rebuilds the texts once the emoji images arrive.
        let _ = TeamsEmojiCatalog.shared.revision
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(MessageRichText.pieces(blocks, size: size).enumerated()), id: \.offset) { index, piece in
                switch piece {
                case .text(let text, let spacing):
                    MessageRichTextView(
                        text: text,
                        unit: inFeedSelection ? messageId.map { FeedSelection.Unit(message: $0, segment: index) } : nil,
                        open: { openURL($0) }
                    )
                    .padding(.top, spacing)
                case .other(let block, let placement):
                    MessageEmbed(
                        block: block, placement: placement,
                        unit: inFeedSelection ? messageId.map { FeedSelection.Unit(message: $0, segment: index) } : nil
                    )
                }
            }
        }
    }

    /// Images drawn in the body, where the HTML puts them.
    static func images(_ blocks: [BlockView]) -> [ImageView] {
        blocks.flatMap { block -> [ImageView] in
            switch block {
            case .image(let image): [image]
            case .list(_, _, let items): items.flatMap { images($0.blocks) }
            case .quote(_, _, _, let body): images(body)
            default: []
            }
        }
    }

    /// One to three emoji alone are drawn large, like Teams.
    static func emojiOnly(_ blocks: [BlockView]) -> Bool {
        guard blocks.count == 1, case .text(let runs) = blocks[0] else { return false }
        let glyphs = runs.map(\.text).joined().filter { !$0.isWhitespace }
        return (1...3).contains(glyphs.count) && glyphs.allSatisfy(\.isEmojiGlyph)
    }
}

/// An image, a card or code between a body's texts, inset as the list or quote it sits in, on that quote's background.
private struct MessageEmbed: View {
    @Environment(\.openURL) private var openURL
    let block: BlockView
    let placement: MessageRichText.Placement
    /// Code's text, for the feed's selection.
    let unit: FeedSelection.Unit?

    var body: some View {
        content
            .padding(.leading, placement.leading)
            .padding(.top, placement.quote?.spacing ?? 0)
            .padding(.bottom, placement.quote.map { $0.bottom ? $0.bottomPadding : 0 } ?? 0)
            .padding(.trailing, placement.quote?.trailing ?? 0)
            .frame(maxWidth: placement.quote == nil ? nil : .infinity, alignment: .leading)
            .background {
                if let quote = placement.quote {
                    UnevenRoundedRectangle(
                        topLeadingRadius: quote.top ? 6 : 0, bottomLeadingRadius: quote.bottom ? 6 : 0,
                        bottomTrailingRadius: quote.bottom ? 6 : 0, topTrailingRadius: quote.top ? 6 : 0
                    )
                    .fill(Theme.input)
                    .overlay(alignment: .leading) {
                        UnevenRoundedRectangle(topLeadingRadius: quote.top ? 6 : 0, bottomLeadingRadius: quote.bottom ? 6 : 0)
                            .fill(Theme.accent)
                            .frame(width: 3)
                    }
                }
            }
            .padding(.top, placement.quote == nil ? placement.spacing : 0)
    }

    @ViewBuilder private var content: some View {
        switch block {
        case .image(let image): PreviewImage(image: image)
        case .card(let card): CardBlock(card: card)
        case .code(let language, let code):
            MessageCode(language: language, code: code, unit: unit, open: { openURL($0) })
        default: EmptyView()
        }
    }
}

/// Code as written: monospaced, never wrapped (scrolls sideways), with its language and a copy button.
private struct MessageCode: View {
    let language: String?
    let code: String
    let unit: FeedSelection.Unit?
    let open: (URL) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language ?? "Code").font(.figtree(11, .semibold)).foregroundStyle(Theme.muted)
                Spacer()
                Button {
                    NSPasteboard.copy(code)
                } label: {
                    Label(L("ui.message_blocks.copy"), systemImage: "doc.on.doc").font(.figtree(11))
                }
                .buttonStyle(GhostButtonStyle())
                .help(L("ui.message_blocks.copy_code"))
            }
            .padding(.leading, 10)
            .padding(.trailing, 4)
            .frame(height: 26)
            Rectangle().fill(Theme.border).frame(height: 1)
            ScrollView(.horizontal) {
                MessageRichTextView(text: MessageRichText.code(code), unit: unit, open: open)
                    .padding(10)
            }
            .fixedSize(horizontal: false, vertical: true)
        }
        .background(Theme.input, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
    }
}

/// A message's author line, selectable along with its text.
struct MessageAuthorLine: View {
    let text: NSAttributedString
    let messageId: String

    var body: some View {
        MessageRichTextView(
            text: text, unit: FeedSelection.Unit(message: messageId, segment: FeedSelection.Unit.authorLine),
            open: { _ in }, singleLine: true
        )
    }
}

private struct MessageRichTextView: NSViewRepresentable {
    let text: NSAttributedString
    let unit: FeedSelection.Unit?
    let open: (URL) -> Void
    var singleLine = false

    func makeNSView(context: Context) -> MessageTextView {
        let view = MessageTextView()
        view.isEditable = false
        view.isSelectable = true
        view.drawsBackground = false
        view.textContainerInset = .zero
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = false
        view.isVerticallyResizable = false
        view.isHorizontallyResizable = false
        view.delegate = context.coordinator
        view.setContentHuggingPriority(.required, for: .vertical)
        if singleLine {
            view.textContainer?.maximumNumberOfLines = 1
            view.textContainer?.lineBreakMode = .byTruncatingTail
        }
        return view
    }

    func updateNSView(_ view: MessageTextView, context: Context) {
        context.coordinator.open = open
        view.linkTextAttributes = [
            .foregroundColor: NSColor(Theme.accent),
            .underlineStyle: NSUnderlineStyle.single.rawValue,
            .cursor: NSCursor.pointingHand,
        ]
        let changed = view.unit != unit || view.textStorage?.isEqual(to: text) != true
        view.unit = unit
        guard changed else { return }
        view.textStorage?.setAttributedString(text)
        view.selection?.register(view)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView view: MessageTextView, context: Context) -> CGSize? {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? 100_000
        return view.fit(width: width)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var open: (URL) -> Void = { _ in }
        func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
            guard let url = link as? URL ?? (link as? String).flatMap(URL.init(string:)) else { return false }
            open(url)
            return true
        }
    }
}

/// TextKit 1, the only layout that draws text blocks and tables.
final class MessageTextView: NSTextView {
    /// Which message text this is, for a selection running across messages.
    var unit: FeedSelection.Unit?

    convenience init() {
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: NSSize(width: 100_000, height: CGFloat.greatestFiniteMagnitude))
        layout.addTextContainer(container)
        self.init(frame: .zero, textContainer: container)
    }

    /// The size the text takes at most `width` wide: its own width when narrower, so a short message still hugs.
    func fit(width: CGFloat) -> CGSize {
        guard let container = textContainer, let layout = layoutManager else { return .zero }
        if container.size.width != width {
            container.size = NSSize(width: width, height: .greatestFiniteMagnitude)
        }
        layout.ensureLayout(for: container)
        // Once laid out again at another width, `usedRect` spans the whole container, which would stretch a short
        // message's bubble; each line's own used rect keeps the text's width.
        var used: CGFloat = 0
        layout.enumerateLineFragments(forGlyphRange: layout.glyphRange(for: container)) { _, line, _, glyphs, _ in
            used = max(used, line.maxX + self.trailingInset(at: layout.characterIndexForGlyph(at: glyphs.location)))
        }
        return CGSize(width: min(width, ceil(used)), height: ceil(layout.usedRect(for: container).height))
    }

    /// The room the boxes around a line take after it; a table takes the whole width, as its columns share it.
    private func trailingInset(at index: Int) -> CGFloat {
        guard let style = textStorage?.attribute(.paragraphStyle, at: index, effectiveRange: nil) as? NSParagraphStyle
        else { return 0 }
        if style.textBlocks.contains(where: { $0 is NSTextTableBlock }) { return .greatestFiniteMagnitude }
        return style.textBlocks.reduce(0) { total, block in
            total + [NSTextBlock.Layer.padding, .border, .margin].reduce(0) { $0 + block.width(for: $1, edge: .maxX) }
        }
    }

    /// The selection across the feed this text takes part in, if any.
    var selection: FeedSelection? {
        guard unit != nil else { return nil }
        return enclosingFeed?.selection
    }

    /// The feed follows a drag from here itself, so it can run across messages.
    override func mouseDown(with event: NSEvent) {
        if unit != nil, let feed = enclosingFeed, feed.press?(event, self) == true { return }
        super.mouseDown(with: event)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        selection?.register(self)
    }

    private var enclosingFeed: FeedTableView? {
        var view = superview
        while let current = view, !(current is FeedTableView) { view = current.superview }
        return view as? FeedTableView
    }

    /// Shows `range` as selected without being the selection, for the texts a selection across messages covers.
    func highlight(_ range: NSRange?) {
        guard let layout = layoutManager, let length = textStorage?.length else { return }
        layout.removeTemporaryAttribute(.backgroundColor, forCharacterRange: NSRange(location: 0, length: length))
        guard let range, range.length > 0 else { return }
        layout.addTemporaryAttribute(.backgroundColor, value: NSColor.selectedTextBackgroundColor, forCharacterRange: range)
    }

    /// Emoji drawn as images copy as the emoji they stand for.
    override func copy(_ sender: Any?) {
        let text = if let selection, selection.spansUnits {
            selection.text()
        } else {
            textStorage.map { MessageRichText.plain($0, range: selectedRange()) } ?? ""
        }
        guard !text.isEmpty else { return }
        NSPasteboard.copy(text)
    }

    override func validateUserInterfaceItem(_ item: any NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(copy(_:)), selection?.spansUnits == true { return true }
        return super.validateUserInterfaceItem(item)
    }

    override func layout() {
        super.layout()
        matchContainerToBounds()
    }

    /// `fit` leaves the container at the width it was asked about, often the whole feed, while the view hugs its text:
    /// a quote's box, as wide as the container, would draw past the bubble (views don't clip to their bounds).
    override func viewWillDraw() {
        matchContainerToBounds()
        super.viewWillDraw()
    }

    private func matchContainerToBounds() {
        if let container = textContainer, container.size.width != bounds.width, bounds.width > 0 {
            container.size = NSSize(width: bounds.width, height: .greatestFiniteMagnitude)
        }
    }
}

/// A quote's or code's box: rounded, filled, stroked, with an optional bar along its leading edge.
final class MessageTextBlock: NSTextBlock {
    var radius: CGFloat = 6
    var fill: NSColor?
    var stroke: NSColor?
    var bar: NSColor?
    /// Cut by an image or a card below, or continuing past one above: square and unstroked on that edge.
    var openTop = false
    var openBottom = false
    /// The quote this box draws, shared with the boxes continuing it; 0 for any other box.
    var group = 0

    override init() {
        super.init()
        // AppKit only asks a block to draw its background when it has a colour, which this drawing replaces.
        backgroundColor = .clear
        // Without a width, a block shrinks to its narrowest line.
        setContentWidth(100, type: .percentageValueType)
    }

    required init?(coder: NSCoder) { super.init(coder: coder) }

    override func drawBackground(
        withFrame frameRect: NSRect, in controlView: NSView?, characterRange: NSRange, layoutManager: NSLayoutManager
    ) {
        let rect = NSRect(
            x: frameRect.minX + width(for: .margin, edge: .minX),
            y: frameRect.minY + width(for: .margin, edge: .minY),
            width: frameRect.width - width(for: .margin, edge: .minX) - width(for: .margin, edge: .maxX),
            height: frameRect.height - width(for: .margin, edge: .minY) - width(for: .margin, edge: .maxY)
        )
        let outline = shape(rect, radius: radius)
        if let fill {
            fill.setFill()
            outline.fill()
        }
        if let bar {
            NSGraphicsContext.saveGraphicsState()
            outline.addClip()
            bar.setFill()
            NSRect(x: rect.minX, y: rect.minY, width: 3, height: rect.height).fill()
            NSGraphicsContext.restoreGraphicsState()
        }
        if let stroke {
            stroke.setStroke()
            let inner = shape(rect.insetBy(dx: 0.5, dy: 0.5), radius: radius)
            inner.lineWidth = 1
            inner.stroke()
        }
    }

    /// A rounded rectangle, square on its open edges. The view is flipped: minY is the top.
    private func shape(_ rect: NSRect, radius: CGFloat) -> NSBezierPath {
        let top = openTop ? 0 : radius
        let bottom = openBottom ? 0 : radius
        let path = NSBezierPath()
        path.move(to: NSPoint(x: rect.minX + top, y: rect.minY))
        path.appendArc(from: NSPoint(x: rect.maxX, y: rect.minY), to: NSPoint(x: rect.maxX, y: rect.maxY), radius: top)
        path.appendArc(from: NSPoint(x: rect.maxX, y: rect.maxY), to: NSPoint(x: rect.minX, y: rect.maxY), radius: bottom)
        path.appendArc(from: NSPoint(x: rect.minX, y: rect.maxY), to: NSPoint(x: rect.minX, y: rect.minY), radius: bottom)
        path.appendArc(from: NSPoint(x: rect.minX, y: rect.minY), to: NSPoint(x: rect.maxX, y: rect.minY), radius: top)
        path.close()
        return path
    }

    override func copy(with zone: NSZone? = nil) -> Any {
        let copy = super.copy(with: zone)
        if let copy = copy as? MessageTextBlock {
            copy.radius = radius
            copy.fill = fill
            copy.stroke = stroke
            copy.bar = bar
            copy.openTop = openTop
            copy.openBottom = openBottom
            copy.group = group
        }
        return copy
    }
}

enum MessageRichText {
    /// A run of text-like blocks drawn as one text, or an image or a card between them.
    enum Piece {
        case text(NSAttributedString, spacing: CGFloat)
        case other(BlockView, Placement)

        /// What the piece gives a selection: its text, or code's.
        @MainActor var selectable: NSAttributedString? {
            switch self {
            case .text(let text, _): text
            case .other(.code(_, let code), _): MessageRichText.code(code)
            case .other: nil
            }
        }
    }

    /// Code's text: monospaced, unwrapped.
    @MainActor static func code(_ code: String) -> NSAttributedString {
        let style = NSMutableParagraphStyle()
        style.lineSpacing = 2
        return NSAttributedString(string: code, attributes: [
            .font: NSFont.monospacedSystemFont(ofSize: 12.5, weight: .regular),
            .foregroundColor: NSColor(Theme.text),
            .paragraphStyle: style,
        ])
    }

    /// Where an image, a card or code sits: its inset, the space above it outside any quote, and the quote around it.
    struct Placement {
        var leading: CGFloat
        var spacing: CGFloat
        var quote: QuoteEdges?
    }

    /// The quote an image or a card cuts, whose box it continues. Where the quote starts or ends with it, its corners
    /// round and it takes the quote's padding.
    struct QuoteEdges {
        /// The space above it inside the quote.
        var spacing: CGFloat
        var bottomPadding: CGFloat
        var trailing: CGFloat
        var top = false
        var bottom = false
        fileprivate let group: Int
    }

    static let blockSpacing: CGFloat = 8
    /// The emoji an inline image stands for.
    static let emoji = NSAttributedString.Key("oxideEmoji")

    /// `range` of `text` as plain text, its emoji images written back as emoji.
    static func plain(_ text: NSAttributedString, range: NSRange) -> String {
        var result = ""
        text.enumerateAttribute(emoji, in: range) { value, part, _ in
            result += (value as? String) ?? (text.string as NSString).substring(with: part)
        }
        return result
    }

    /// The blocks as texts and the images and cards between them; `size` is the base for runs without their own.
    @MainActor static func pieces(_ blocks: [BlockView], size: Double) -> [Piece] {
        var builder = Builder(base: size)
        builder.blocks(blocks, context: Context(indent: 0, textBlocks: [], color: NSColor(Theme.text), size: size))
        return builder.finish()
    }

    struct Context {
        var indent: CGFloat
        var textBlocks: [NSTextBlock]
        var color: NSColor
        var size: Double
    }

    @MainActor struct Builder {
        let base: Double
        private var output = NSMutableAttributedString()
        private var pieces: [Piece] = []
        /// The space above the current text, outside it: TextKit drops a first paragraph's spacing before.
        private var textSpacing: CGFloat = 0
        /// Where each block starts, to space it from the one before.
        private var pendingSpacing: CGFloat = 0
        /// The boxes cut by an image or a card, and the boxes continuing them in the next text.
        private var continued: [ObjectIdentifier: NSTextBlock] = [:]
        /// The quotes holding a paragraph, an image or a card already.
        private var filled: Set<Int> = []
        private var nextGroup = 1

        init(base: Double) { self.base = base }

        mutating func finish() -> [Piece] {
            closeText()
            // A quote's background rounds above and below an image or card only where the quote has nothing else.
            for index in pieces.indices {
                guard case .other(let block, var placement) = pieces[index], var quote = placement.quote else { continue }
                quote.top = !sameQuote(pieces, index - 1, quote.group, atEnd: true)
                quote.bottom = !sameQuote(pieces, index + 1, quote.group, atEnd: false)
                placement.quote = quote
                pieces[index] = .other(block, placement)
            }
            return pieces
        }

        /// Whether the piece at `index` draws quote `group` at its end or its start.
        private func sameQuote(_ pieces: [Piece], _ index: Int, _ group: Int, atEnd: Bool) -> Bool {
            guard pieces.indices.contains(index) else { return false }
            switch pieces[index] {
            case .other(_, let placement):
                return placement.quote?.group == group
            case .text(let text, _):
                guard text.length > 0 else { return false }
                let style = text.attribute(.paragraphStyle, at: atEnd ? text.length - 1 : 0, effectiveRange: nil)
                    as? NSParagraphStyle
                return style?.textBlocks.contains { ($0 as? MessageTextBlock)?.group == group } ?? false
            }
        }

        /// Ends the current text; an empty one, as before an image opening a message, draws nothing.
        private mutating func closeText() {
            // The text ends with the last block's newline, which would draw an empty line.
            if output.string.hasSuffix("\n") { output.deleteCharacters(in: NSRange(location: output.length - 1, length: 1)) }
            if output.length > 0 { pieces.append(.text(output, spacing: textSpacing)) }
            output = NSMutableAttributedString()
            textSpacing = 0
        }

        /// Cuts the text for an image or a card. Inside a quote, its box goes on around it and in the next text, open
        /// where it was cut; elsewhere the spacing above it and above the next text sits between the views.
        private mutating func embed(_ block: BlockView, context: Context) {
            let boxes = context.textBlocks.map(current).compactMap { $0 as? MessageTextBlock }
            let quote = boxes.first { $0.bar != nil }
            let leading = context.indent + boxes.reduce(0) { $0 + $1.width(for: .padding, edge: .minX) }
            // A quote opening with the image or card leaves it the space above its box, and its top padding.
            let opening = quote.map { !filled.contains($0.group) } ?? false
            let quoteEdges = quote.map {
                QuoteEdges(
                    spacing: opening ? $0.width(for: .padding, edge: .minY) : pendingSpacing,
                    bottomPadding: $0.width(for: .padding, edge: .maxY),
                    trailing: $0.width(for: .padding, edge: .maxX),
                    group: $0.group
                )
            }
            if let quote { filled.insert(quote.group) }
            for box in boxes {
                let next = box.copy() as! MessageTextBlock
                box.openBottom = true
                box.setWidth(0, type: .absoluteValueType, for: .padding, edge: .maxY)
                next.openTop = true
                next.setWidth(0, type: .absoluteValueType, for: .padding, edge: .minY)
                next.setWidth(0, type: .absoluteValueType, for: .margin, edge: .minY)
                continued[ObjectIdentifier(box)] = next
            }
            closeText()
            let outside = opening ? quote?.width(for: .margin, edge: .minY) ?? 0 : pendingSpacing
            pieces.append(.other(block, Placement(
                leading: leading,
                spacing: pieces.isEmpty || (quote != nil && !opening) ? 0 : outside,
                quote: quoteEdges
            )))
        }

        /// The box a paragraph goes in now: `block`, or what continues it past an image or a card.
        private func current(_ block: NSTextBlock) -> NSTextBlock {
            var block = block
            while let next = continued[ObjectIdentifier(block)] { block = next }
            return block
        }

        mutating func blocks(_ blocks: [BlockView], context: Context) {
            for block in blocks { self.block(block, context: context) }
        }

        mutating func block(_ block: BlockView, context: Context) {
            switch block {
            case .text(let runs):
                paragraph(runs, context: context)
            case .list(let ordered, let start, let items):
                list(ordered: ordered, start: Int(start), items: items, context: context)
            case .quote(let author, let time, let messageId, let body):
                quote(author: author, time: time, reply: messageId != nil || author != nil, body: body, context: context)
            case .table(let header, let rows):
                table(header: header, rows: rows, context: context)
            case .rule:
                rule(context: context)
            case .image, .card, .code:
                embed(block, context: context)
            }
            pendingSpacing = MessageRichText.blockSpacing
        }

        // MARK: Blocks

        private mutating func paragraph(_ runs: [RunView], context: Context, prefix: NSAttributedString? = nil) {
            let start = output.length
            if let prefix { output.append(prefix) }
            for run in runs { append(run, context: context) }
            output.append(NSAttributedString(string: "\n", attributes: [.font: font(size: context.size)]))
            style(NSRange(location: start, length: output.length - start), context: context)
        }

        private mutating func list(ordered: Bool, start: Int, items: [ListItemView], context: Context) {
            let markerWidth: CGFloat = 14
            let gap: CGFloat = 6
            for (index, item) in items.enumerated() {
                var inner = context
                inner.indent = context.indent + 2 + markerWidth + gap
                let marker = NSMutableAttributedString(
                    string: "\t" + (ordered ? "\(start + index)." : "•") + "\t",
                    attributes: [.font: font(size: context.size), .foregroundColor: NSColor(Theme.textSoft)]
                )
                let first = output.length
                if index > 0 { pendingSpacing = 4 }
                var blocks = item.blocks
                if case .text(let runs)? = blocks.first {
                    blocks.removeFirst()
                    paragraph(runs, context: inner, prefix: marker)
                } else {
                    paragraph([], context: inner, prefix: marker)
                }
                hang(NSRange(location: first, length: output.length - first), from: context.indent + 2, marker: markerWidth, at: inner.indent)
                for block in blocks {
                    pendingSpacing = 4
                    self.block(block, context: inner)
                }
            }
        }

        private mutating func quote(author: String?, time: String?, reply: Bool, body: [BlockView], context: Context) {
            let box = MessageTextBlock()
            box.radius = 6
            box.fill = NSColor(Theme.input)
            box.bar = NSColor(Theme.accent)
            box.group = nextGroup
            nextGroup += 1
            pad(box, top: 6, leading: 12, bottom: 6, trailing: 10)
            space(box)
            var inner = context
            inner.textBlocks.append(box)
            inner.color = NSColor(Theme.textSoft)
            inner.size = 13
            if author != nil || time != nil {
                let header = NSMutableAttributedString()
                if let author {
                    header.append(NSAttributedString(string: author, attributes: [
                        .font: font(size: 12, bold: true), .foregroundColor: NSColor(Theme.text),
                    ]))
                }
                if let time {
                    header.append(NSAttributedString(string: (author == nil ? "" : "  ") + time, attributes: [
                        .font: font(size: 11), .foregroundColor: NSColor(Theme.muted),
                    ]))
                }
                paragraph([], context: inner, prefix: header)
                pendingSpacing = 3
            }
            if reply {
                // A reply shows the start of the message it answers, as Teams does, not the whole of it.
                let excerpt = MessageRichText.excerpt(body, limit: 280)
                paragraph([RunView(text: excerpt, link: nil, bold: false, italic: false, code: false, underline: false,
                                   strike: false, color: nil, highlight: nil, size: 0, mention: false)], context: inner)
            } else {
                for block in body { self.block(block, context: inner) }
            }
        }

        private mutating func table(header: Bool, rows: [TableRowView], context: Context) {
            let columns = rows.map(\.cells.count).max() ?? 0
            guard columns > 0 else { return }
            let table = NSTextTable()
            table.numberOfColumns = columns
            table.collapsesBorders = true
            table.hidesEmptyCells = false
            space(table)
            for (rowIndex, row) in rows.enumerated() {
                for column in 0..<columns {
                    let cell = NSTextTableBlock(table: table, startingRow: rowIndex, rowSpan: 1, startingColumn: column, columnSpan: 1)
                    cell.setWidth(1, type: .absoluteValueType, for: .border)
                    cell.setBorderColor(NSColor(Theme.border))
                    pad(cell, top: 6, leading: 10, bottom: 6, trailing: 10)
                    if header && rowIndex == 0 { cell.backgroundColor = NSColor(Theme.input) }
                    var inner = context
                    inner.textBlocks.append(cell)
                    inner.size = context.size - 1
                    paragraph(column < row.cells.count ? row.cells[column].runs : [], context: inner)
                }
            }
        }

        private mutating func rule(context: Context) {
            let line = MessageTextBlock()
            line.radius = 0
            line.fill = NSColor(Theme.border)
            line.setValue(1, type: .absoluteValueType, for: .height)
            line.setValue(1, type: .absoluteValueType, for: .maximumHeight)
            space(line)
            var inner = context
            inner.textBlocks.append(line)
            let start = output.length
            output.append(NSAttributedString(string: "\u{200B}\n", attributes: [.font: font(size: 1)]))
            style(NSRange(location: start, length: output.length - start), context: inner, lineSpacing: 0)
        }

        // MARK: Runs

        private func append(_ run: RunView, context: Context) {
            let size = run.size == 0 ? (run.code ? context.size - 1 : context.size) : Double(run.size)
            var attributes: [NSAttributedString.Key: Any] = [
                .font: run.code
                    ? NSFont.monospacedSystemFont(ofSize: size, weight: run.bold ? .semibold : .regular)
                    : font(size: size, bold: run.bold || run.mention, italic: run.italic),
                .foregroundColor: ComposerText.color(run.color) ?? context.color,
            ]
            if run.underline { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
            if run.strike { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            if let color = ComposerText.color(run.highlight) {
                attributes[.backgroundColor] = color
            } else if run.code {
                attributes[.backgroundColor] = NSColor(Theme.input)
            }
            if run.mention { attributes[.foregroundColor] = NSColor(Theme.accentText) }
            if let link = run.link, let url = URL(string: link) { attributes[.link] = url }
            guard !run.code, run.link == nil else {
                output.append(NSAttributedString(string: run.text, attributes: attributes))
                return
            }
            var buffer = ""
            for glyph in run.text {
                guard let item = TeamsEmojiCatalog.emoji(for: glyph),
                      let image = MessageEmojiImage.cached(item.id).sized(size * 1.2) else {
                    buffer.append(glyph)
                    continue
                }
                if !buffer.isEmpty { output.append(NSAttributedString(string: buffer, attributes: attributes)) }
                buffer = ""
                let side = size * 1.2
                let attachment = NSTextAttachment()
                attachment.image = image
                attachment.bounds = CGRect(x: 0, y: -side * 0.18, width: side, height: side)
                let piece = NSMutableAttributedString(attachment: attachment)
                piece.addAttributes(attributes, range: NSRange(location: 0, length: piece.length))
                piece.addAttribute(MessageRichText.emoji, value: String(glyph), range: NSRange(location: 0, length: piece.length))
                output.append(piece)
            }
            if !buffer.isEmpty { output.append(NSAttributedString(string: buffer, attributes: attributes)) }
        }

        // MARK: Layout

        /// Every paragraph of `range` gets the context's indent and boxes; the first one the space before its block.
        private mutating func style(_ range: NSRange, context: Context, lineSpacing: CGFloat? = nil) {
            let paragraph = NSMutableParagraphStyle()
            paragraph.lineSpacing = lineSpacing ?? (context.size > 20 ? 0 : 4)
            paragraph.firstLineHeadIndent = context.indent
            paragraph.headIndent = context.indent
            paragraph.textBlocks = context.textBlocks.map(current)
            for case let box as MessageTextBlock in paragraph.textBlocks where box.group != 0 { filled.insert(box.group) }
            output.addAttribute(.paragraphStyle, value: paragraph, range: range)
            if range.location == 0, !pieces.isEmpty {
                // A text after an image or a card: the space goes above it, or atop the quote it goes on with.
                let quote = paragraph.textBlocks.lazy.compactMap { $0 as? MessageTextBlock }.first { $0.openTop && $0.group != 0 }
                if let quote {
                    quote.setWidth(pendingSpacing, type: .absoluteValueType, for: .padding, edge: .minY)
                } else {
                    textSpacing = pendingSpacing
                }
                pendingSpacing = 0
            }
            if pendingSpacing > 0 {
                let first = (output.string as NSString).paragraphRange(for: NSRange(location: range.location, length: 0))
                let spaced = paragraph.mutableCopy() as! NSMutableParagraphStyle
                spaced.paragraphSpacingBefore = pendingSpacing
                output.addAttribute(.paragraphStyle, value: spaced, range: first)
                pendingSpacing = 0
            }
        }

        /// A list item's first line starts at its marker, right-aligned in `marker` points; the rest hangs at `indent`.
        private func hang(_ range: NSRange, from start: CGFloat, marker: CGFloat, at indent: CGFloat) {
            let first = (output.string as NSString).paragraphRange(for: NSRange(location: range.location, length: 0))
            guard let current = output.attribute(.paragraphStyle, at: first.location, effectiveRange: nil) as? NSParagraphStyle,
                  let style = current.mutableCopy() as? NSMutableParagraphStyle else { return }
            style.firstLineHeadIndent = start
            style.headIndent = indent
            style.tabStops = [
                NSTextTab(textAlignment: .right, location: start + marker),
                NSTextTab(textAlignment: .left, location: indent),
            ]
            output.addAttribute(.paragraphStyle, value: style, range: first)
        }

        /// A box takes the space before it outside its edge, where a paragraph would take it inside.
        private mutating func space(_ block: NSTextBlock) {
            block.setWidth(pendingSpacing, type: .absoluteValueType, for: .margin, edge: .minY)
            pendingSpacing = 0
        }

        private func pad(_ block: NSTextBlock, top: CGFloat, leading: CGFloat, bottom: CGFloat, trailing: CGFloat) {
            block.setWidth(top, type: .absoluteValueType, for: .padding, edge: .minY)
            block.setWidth(leading, type: .absoluteValueType, for: .padding, edge: .minX)
            block.setWidth(bottom, type: .absoluteValueType, for: .padding, edge: .maxY)
            block.setWidth(trailing, type: .absoluteValueType, for: .padding, edge: .maxX)
        }

        private func font(size: Double, bold: Bool = false, italic: Bool = false) -> NSFont {
            MessageRichText.font(size: size, bold: bold, italic: italic)
        }
    }

    /// Figtree when the bundle registered it, as `Font.figtree`; the system font otherwise.
    static func font(size: Double, bold: Bool = false, italic: Bool = false) -> NSFont {
        let weight: NSFont.Weight = bold ? .semibold : .regular
        var font = NSFont(descriptor: NSFontDescriptor(fontAttributes: [
            .family: "Figtree", .traits: [NSFontDescriptor.TraitKey.weight: weight.rawValue],
        ]), size: size)
        if font?.familyName != "Figtree" { font = .systemFont(ofSize: size, weight: weight) }
        guard var result = font else { return .systemFont(ofSize: size) }
        if italic { result = NSFontManager.shared.convert(result, toHaveTrait: .italicFontMask) }
        return result
    }

    /// The text of `blocks` on one line, cut at `limit` characters.
    static func excerpt(_ blocks: [BlockView], limit: Int) -> String {
        func text(_ block: BlockView) -> String {
            switch block {
            case .text(let runs): runs.map(\.text).joined()
            case .list(_, _, let items): items.flatMap(\.blocks).map(text).joined(separator: " ")
            case .quote(_, _, _, let body): body.map(text).joined(separator: " ")
            case .code(_, let code): code
            case .table(_, let rows): rows.flatMap(\.cells).map { $0.runs.map(\.text).joined() }.joined(separator: " ")
            default: ""
            }
        }
        let flat = blocks.map(text).joined(separator: " ").split(whereSeparator: \.isNewline).joined(separator: " ")
        return flat.count > limit ? String(flat.prefix(limit)) + "…" : flat
    }
}
