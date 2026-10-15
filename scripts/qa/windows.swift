// Lists on-screen windows of the given owner names, front to back:
//   id layer order x y w h owner
// Used by ui-glue-smoke.sh to capture only the launcher's own window (screencapture -l)
// and to check which window is drawn above which, without photographing the desktop.
import CoreGraphics
import Foundation

let owners = Set(CommandLine.arguments.dropFirst())
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for (order, w) in list.enumerated() {
    guard let owner = w[kCGWindowOwnerName as String] as? String, owners.contains(owner) else { continue }
    let id = w[kCGWindowNumber as String] as? Int ?? 0
    let layer = w[kCGWindowLayer as String] as? Int ?? 0
    let b = w[kCGWindowBounds as String] as? [String: Double] ?? [:]
    print(id, layer, order, Int(b["X"] ?? 0), Int(b["Y"] ?? 0), Int(b["Width"] ?? 0), Int(b["Height"] ?? 0), owner)
}
