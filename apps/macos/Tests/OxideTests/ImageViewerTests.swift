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
}
