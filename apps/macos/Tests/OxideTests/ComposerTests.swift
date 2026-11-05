import XCTest
import AppKit
import OxideCore
@testable import Oxide

final class ComposerTests: XCTestCase {
    func testAnImageAloneIsPastedAsAnImageAndTextWithPicturesAsText() throws {
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("oxide.tests.\(UUID().uuidString)"))
        defer { pasteboard.releaseGlobally() }
        let png = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8,
            samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)?
            .representation(using: .png, properties: [:]))
        pasteboard.declareTypes([.png], owner: nil)
        pasteboard.setData(png, forType: .png)
        XCTAssertEqual(ComposerTextView.pastedImage(from: pasteboard), png)
        pasteboard.declareTypes([.png, .string], owner: nil)
        pasteboard.setData(png, forType: .png)
        pasteboard.setString("Légende", forType: .string)
        XCTAssertNil(ComposerTextView.pastedImage(from: pasteboard))
    }
    func testPasteIsOfferedForAnImageAndHandsItOver() throws {
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("oxide.tests.\(UUID().uuidString)"))
        defer { pasteboard.releaseGlobally() }
        let image = NSImage(size: NSSize(width: 4, height: 3), flipped: false) { rect in
            NSColor.red.setFill()
            rect.fill()
            return true
        }
        let png = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(image.tiffRepresentation))?.representation(using: .png, properties: [:]))
        pasteboard.declareTypes([.png], owner: nil)
        pasteboard.setData(png, forType: .png)
        let text = ComposerTextView(frame: NSRect(x: 0, y: 0, width: 200, height: 40))
        text.importsGraphics = false
        text.pasteboard = pasteboard
        var pasted: (id: String, png: Data)?
        text.imagePaste = { id, png, _ in pasted = (id, png) }
        let item = NSMenuItem(title: "", action: #selector(NSText.paste(_:)), keyEquivalent: "")
        XCTAssertTrue(text.validateUserInterfaceItem(item))
        text.paste(nil)
        XCTAssertEqual(pasted?.png, png)
        let placed = text.textStorage?.attribute(.attachment, at: 0, effectiveRange: nil) as? PastedImageAttachment
        XCTAssertEqual(placed?.id, pasted?.id)
    }
    func testAPastedImageKeepsItsIdThroughTheDraftFormatsAndShowsAtMost240By120() {
        PastedImageAttachment.images["img"] = NSImage(size: NSSize(width: 800, height: 200))
        let image = ComposerFormat(start: 2, length: 1, bold: false, italic: false, underline: false, strike: false, code: false, color: nil, highlight: nil, size: 14, link: nil, block: .paragraph, indent: 0, alignment: "left", image: "img")
        let text = NSTextView()
        text.textStorage?.setAttributedString(ComposerText.attributed("a \u{FFFC} b", formats: [image]))
        let attachment = text.textStorage?.attribute(.attachment, at: 2, effectiveRange: nil) as? PastedImageAttachment
        XCTAssertEqual(attachment?.bounds.size, CGSize(width: 240, height: 60))
        let runs = ComposerText.formats(text)
        XCTAssertEqual(runs.filter { $0.image != nil }.map { [$0.start, $0.length] }, [[2, 1]])
        XCTAssertEqual(runs.first { $0.image != nil }?.image, "img")
    }
    func testTheCaretBesideAnImageKeepsToTheTextHeightAtTheBottomOfTheLine() {
        let layout = NSLayoutManager()
        let text = layout.defaultLineHeight(for: ComposerText.baseFont)
        let caret = ComposerTextView.caret(in: NSRect(x: 10, y: 4, width: 1, height: 120), font: ComposerText.baseFont, layout: layout)
        XCTAssertEqual(caret, NSRect(x: 10, y: 124 - text, width: 1, height: text))
        let plain = NSRect(x: 10, y: 4, width: 1, height: text)
        XCTAssertEqual(ComposerTextView.caret(in: plain, font: ComposerText.baseFont, layout: layout), plain)
    }
    func testOnlyDroppedFilesAreTakenAsAttachments() {
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("oxide.tests.\(UUID().uuidString)"))
        defer { pasteboard.releaseGlobally() }
        pasteboard.clearContents()
        pasteboard.writeObjects([URL(fileURLWithPath: "/tmp/rapport.pdf") as NSURL])
        XCTAssertEqual(ComposerTextView.droppedFiles(from: pasteboard), ["/tmp/rapport.pdf"])
        pasteboard.clearContents()
        pasteboard.writeObjects([URL(string: "https://example.com/rapport.pdf")! as NSURL, "texte" as NSString])
        XCTAssertEqual(ComposerTextView.droppedFiles(from: pasteboard), [])
    }
    func testMentionDetectionAtCaretUsesUTF16AndRejectsEmailAndSelections() {
        let text = "😀 Bonjour @Édou fin"
        let caret = ("😀 Bonjour @Édou" as NSString).length
        let query = MentionQuery.detect(text, selection: NSRange(location: caret, length: 0))
        XCTAssertEqual(query?.query, "Édou")
        XCTAssertEqual(query?.range, NSRange(location: 11, length: 5))
        XCTAssertNil(MentionQuery.detect("a@example.com", selection: NSRange(location: 13, length: 0)))
        XCTAssertNil(MentionQuery.detect("@al", selection: NSRange(location: 1, length: 1)))
        XCTAssertNil(MentionQuery.detect("@al\nbonjour", selection: NSRange(location: 11, length: 0)))
    }
    func testEscapeDismissesUntilQueryChangesAndSelectedMentionDoesNotReopen() {
        let input = DraftInput()
        input.capture("@al", selection: NSRange(location: 3, length: 0))
        XCTAssertNotNil(input.mention)
        input.dismiss()
        input.capture("@al", selection: NSRange(location: 3, length: 0))
        XCTAssertNil(input.mention)
        input.capture("@ali", selection: NSRange(location: 4, length: 0))
        XCTAssertEqual(input.mention?.query, "ali")
        input.dismiss()
        input.capture("@Alice ", selection: NSRange(location: 7, length: 0), mentions: [MentionRange(start: 0, length: 6)], coreChange: true)
        XCTAssertNil(input.mention)
        input.capture("@Alice @bo", selection: NSRange(location: 10, length: 0), mentions: [MentionRange(start: 0, length: 6)])
        XCTAssertEqual(input.mention?.query, "bo")
    }
    @MainActor func testAReplyQuotesItsLineThenBreaksItWherePlainTextFollows() {
        var quote = ComposerText.empty
        quote.block = .quote
        XCTAssertEqual(Store.replyEdits(quoting: "Bob 😀: ok"), [
            .replaceDraft(start: 0, length: 0, text: "Bob 😀: ok", mention: nil, emoji: nil, style: quote),
            .replaceDraft(start: 10, length: 0, text: "\n", mention: nil, emoji: nil, style: ComposerText.empty),
        ])
    }
}

extension ComposerTests {
    func testRichTextRoundTripAndFormattingRetainsUTF16Ranges() {
        let original = "😀 Bonjour lien"
        let format = ComposerFormat(start: 3, length: 7, bold: true, italic: true, underline: true, strike: true, code: false, color: "#c4314b", highlight: "#fff100", size: 18, link: nil, block: .paragraph, indent: 0, alignment: "left", image: nil)
        let text = NSTextView()
        text.textStorage?.setAttributedString(ComposerText.attributed(original, formats: [format]))
        let runs = ComposerText.formats(text)
        let styled = runs.first { $0.start == 3 }!
        XCTAssertEqual(styled.length, 7)
        XCTAssertTrue(styled.bold && styled.italic && styled.underline && styled.strike)
        XCTAssertEqual(styled.color, "#c4314b")
        XCTAssertEqual(styled.highlight, "#fff100")
        XCTAssertEqual(styled.size, 18)
        XCTAssertEqual(ComposerText.attributed(text.string, formats: runs).string, original)
    }
    func testToolbarChangesOnlySelectionAndCodeListsAndLinksSerialize() {
        let text = NSTextView()
        text.isRichText = true
        text.textStorage?.setAttributedString(ComposerText.attributed("alpha beta\ngamma", formats: []))
        let input = DraftInput()
        input.textView = text
        input.selection = NSRange(location: 6, length: 4)
        text.setSelectedRange(input.selection)
        input.toggle(.bold)
        XCTAssertFalse(ComposerText.formats(text)[0].bold)
        XCTAssertTrue(ComposerText.formats(text).contains { $0.start == 6 && $0.bold })
        input.setBlock(.numbered)
        XCTAssertEqual(ComposerText.formats(text)[0].block, .numbered)
        XCTAssertEqual(ComposerText.formats(text).last?.block, .paragraph)
        input.insertLink(title: "beta", url: "https://example.test/?a=1&b=2")
        XCTAssertTrue(ComposerText.formats(text).contains { $0.link == "https://example.test/?a=1&b=2" })
        input.toggle(.code)
        XCTAssertTrue(ComposerText.formats(text).contains { $0.code })
    }
    func testTypingStyleWorksInEmptyEditorAndClears() {
        let text = NSTextView()
        text.typingAttributes = ComposerText.base
        let input = DraftInput()
        input.textView = text
        input.toggle(.italic)
        XCTAssertTrue(input.currentFormat.italic)
        input.clearFormatting()
        XCTAssertFalse(input.currentFormat.italic)
        input.setBlock(.bullet)
        XCTAssertEqual(input.currentFormat.block, .bullet)
    }
}

extension ComposerTests {
    func testAutomaticSpellingCorrectionToggledInOneComposerCarriesToTheNext() {
        let suite = "ComposerTests.\(name)"
        let preferences = UserDefaults(suiteName: suite)!
        defer { preferences.removePersistentDomain(forName: suite) }
        let first = ComposerTextView()
        first.preferences = preferences
        first.restoreAutomaticSpellingCorrection()
        let initial = first.isAutomaticSpellingCorrectionEnabled

        first.toggleAutomaticSpellingCorrection(nil)
        let next = ComposerTextView()
        next.preferences = preferences
        next.restoreAutomaticSpellingCorrection()

        XCTAssertEqual(next.isAutomaticSpellingCorrectionEnabled, !initial)
    }
}
