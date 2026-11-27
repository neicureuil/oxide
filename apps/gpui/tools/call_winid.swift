// call_winid <pid> [main]: prints the id of a process's on-screen call window (narrower than the 1180-point main
// window), else its first window; with `main`, the main window. For `screencapture -l`.
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
let mine = list.filter { ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
let width = { (w: [String: Any]) -> Double in ((w[kCGWindowBounds as String] as? [String: Any])?["Width"] as? Double) ?? 0 }
let wantMain = CommandLine.arguments.count > 2 && CommandLine.arguments[2] == "main"
if let call = mine.first(where: { wantMain ? width($0) >= 1100 : width($0) < 1100 }) ?? mine.first {
    print(call[kCGWindowNumber as String]!)
    exit(0)
}
exit(1)
