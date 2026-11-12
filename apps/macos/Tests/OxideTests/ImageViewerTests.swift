import XCTest
@testable import Oxide

final class ImageViewerTests: XCTestCase {
    /// 64 points of margin left and right, 24 above and below.
    private let area = CGSize(width: 1128, height: 548)

    func testALargeImageShrinksToTheAreaLessItsMargins() {
        XCTAssertEqual(ImageViewer.fittedSize(of: CGSize(width: 4000, height: 1000), in: area), CGSize(width: 1000, height: 250))
        XCTAssertEqual(ImageViewer.fittedSize(of: CGSize(width: 1000, height: 2000), in: area), CGSize(width: 250, height: 500))
    }

    func testASmallImageIsEnlargedTwiceAtMost() {
        XCTAssertEqual(ImageViewer.fittedSize(of: CGSize(width: 100, height: 50), in: area), CGSize(width: 200, height: 100))
    }

    func testAnImageWithoutSizeTakesTheWholeBox() {
        XCTAssertEqual(ImageViewer.fittedSize(of: .zero, in: area), CGSize(width: 1000, height: 500))
    }

    /// Teams names a pasted file after it; its own placeholder alt texts give a generic name.
    func testTheSavedFileIsNamedAfterTheAltText() {
        XCTAssertEqual(ImageViewer.fileName(for: "Capture.jpg"), "Capture.png")
        XCTAssertEqual(ImageViewer.fileName(for: "a/b:c.png"), "a-b-c.png")
        XCTAssertEqual(ImageViewer.fileName(for: "Image"), "Image.png")
        XCTAssertEqual(ImageViewer.fileName(for: "image partagée"), "Image.png")
    }

    /// The point under the pointer stays under it while zooming in.
    func testZoomingInKeepsTheAnchorInPlace() {
        let result = ImageViewer.zoomed(to: 2, from: 1, offset: .zero, anchor: CGSize(width: 100, height: -50))
        XCTAssertEqual(result.zoom, 2)
        XCTAssertEqual(result.offset, CGSize(width: -100, height: 50))
        let further = ImageViewer.zoomed(to: 4, from: 2, offset: result.offset, anchor: CGSize(width: 100, height: -50))
        XCTAssertEqual(further.offset, CGSize(width: -300, height: 150))
    }

    func testTheZoomStaysWithinItsBoundsAndRecentresWhenNoLargerThanFitted() {
        XCTAssertEqual(ImageViewer.zoomed(to: 20, from: 1, offset: .zero, anchor: .zero).zoom, 8)
        let out = ImageViewer.zoomed(to: 0.1, from: 2, offset: CGSize(width: 40, height: 40), anchor: CGSize(width: 10, height: 10))
        XCTAssertEqual(out.zoom, 0.25)
        XCTAssertEqual(out.offset, .zero)
    }
}
