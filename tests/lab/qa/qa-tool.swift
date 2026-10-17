// qa-tool: small macOS helpers for the lab QA scripts (compiled in the guest with swiftc on first use).
//   qa-tool click X Y [right|ctrl|double]       mouse click at screen point
//   qa-tool keyrepeat CODE COUNT [MS]           key down with the auto-repeat flag COUNT times (a held key), then key up
//   qa-tool pb-set TEXT [concealed|transient|both]   put TEXT on the general pasteboard (+ nspasteboard.org marker types)
//   qa-tool pb-set-file PATH                     put a file reference on the pasteboard
//   qa-tool pb-types                             print the types now on the pasteboard
//   qa-tool hog KEYCODE MODS SECONDS             register a global hotkey (Carbon) and hold it; MODS: cmd=256 shift=512 opt=2048 ctrl=4096 (sum)
//   qa-tool front                                print the frontmost application's bundle id
import AppKit
import Carbon
import CoreGraphics
import Foundation

let a = CommandLine.arguments
func die(_ m: String) -> Never { FileHandle.standardError.write(Data((m + "\n").utf8)); exit(2) }
guard a.count > 1 else { die("usage: qa-tool click|keyrepeat|pb-set|pb-set-file|pb-types|hog|front ...") }

switch a[1] {
case "click":
    let p = CGPoint(x: Double(a[2])!, y: Double(a[3])!)
    let mode = a.count > 4 ? a[4] : "left"
    func post(_ t: CGEventType, _ b: CGMouseButton, _ f: CGEventFlags = [], clicks: Int64 = 1) {
        let e = CGEvent(mouseEventSource: nil, mouseType: t, mouseCursorPosition: p, mouseButton: b)!
        e.flags = f
        e.setIntegerValueField(.mouseEventClickState, value: clicks)
        e.post(tap: .cghidEventTap)
        usleep(80_000)
    }
    post(.mouseMoved, .left)
    switch mode {
    case "right": post(.rightMouseDown, .right); post(.rightMouseUp, .right)
    case "ctrl": post(.leftMouseDown, .left, .maskControl); post(.leftMouseUp, .left, .maskControl)
    case "double":
        post(.leftMouseDown, .left); post(.leftMouseUp, .left)
        post(.leftMouseDown, .left, clicks: 2); post(.leftMouseUp, .left, clicks: 2)
    default: post(.leftMouseDown, .left); post(.leftMouseUp, .left)
    }
case "keyrepeat":
    let code = CGKeyCode(UInt16(a[2])!)
    let n = Int(a[3])!
    let ms = a.count > 4 ? UInt32(a[4])! : 30
    for i in 0..<n {
        let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)!
        e.setIntegerValueField(.keyboardEventAutorepeat, value: i == 0 ? 0 : 1)
        e.post(tap: .cghidEventTap)
        usleep(ms * 1000)
    }
    CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)!.post(tap: .cghidEventTap)
case "pb-set":
    let pb = NSPasteboard.general
    pb.clearContents()
    let mode = a.count > 3 ? a[3] : ""
    var types: [NSPasteboard.PasteboardType] = [.string]
    if mode == "concealed" || mode == "both" { types.append(NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType")) }
    if mode == "transient" || mode == "both" { types.append(NSPasteboard.PasteboardType("org.nspasteboard.TransientType")) }
    pb.declareTypes(types, owner: nil)
    pb.setString(a[2], forType: .string)
    for t in types where t != .string { pb.setString("", forType: t) }
case "pb-set-file":
    let pb = NSPasteboard.general
    pb.clearContents()
    pb.writeObjects([URL(fileURLWithPath: a[2]) as NSURL])
case "pb-types":
    for t in NSPasteboard.general.types ?? [] { print(t.rawValue) }
case "hog":
    var ref: EventHotKeyRef?
    let id = EventHotKeyID(signature: OSType(0x51414854), id: 1)
    let st = RegisterEventHotKey(UInt32(a[2])!, UInt32(a[3])!, id, GetApplicationEventTarget(), 0, &ref)
    print("RegisterEventHotKey status \(st)")
    fflush(stdout)
    RunLoop.current.run(until: Date(timeIntervalSinceNow: Double(a[4])!))
case "front":
    print(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "none")
default: die("unknown command \(a[1])")
}
