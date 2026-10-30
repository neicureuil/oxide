import XCTest
import AppKit
import OxideCore
@testable import Oxide

final class ComposerTests: XCTestCase {
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
}

extension ComposerTests {
    func testRichTextRoundTripAndFormattingRetainsUTF16Ranges() {
        let original = "😀 Bonjour lien"
        let format = ComposerFormat(start: 3, length: 7, bold: true, italic: true, underline: true, strike: true, code: false, color: "#c4314b", highlight: "#fff100", size: 18, link: nil, block: .paragraph, indent: 0, alignment: "left")
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
