// Measurement helper for scripts/qa/perf-gui.sh. Needs the desktop (reads the window list; the
// hotkey/type/key commands post synthetic key events and need Accessibility permission for the
// terminal that runs it). Never run it outside the desktop lock.
//
//   perf-probe visible <owner> <min-width> <timeout-ms>
//       poll the on-screen window list every ~1 ms until a window of <owner> at least <min-width>
//       points wide is on screen; print "visible <unix-seconds>"; exit 1 on timeout.
//       (The status item's own window is small, so the width filter ignores it.)
//   perf-probe hidden <owner> <min-width> <timeout-ms>
//       same, until no such window is on screen.
//   perf-probe hotkey <keycode> <flags-csv> <owner> <min-width> <timeout-ms>
//       post the key chord (flags: alt,shift,ctrl,cmd), then like "visible"; prints
//       "post <t0>" and "visible <t1>" and "latency_ms <t1-t0>".
//   perf-probe key <keycode>            post one key down+up (53 = Esc)
//   perf-probe type <text>              post the characters as unicode key events
//   perf-probe bounds <owner>           print "x y w h layer" of every on-screen window of <owner>
//   perf-probe trusted                  print whether this process may post events
import ApplicationServices
import CoreGraphics
import Foundation

func now() -> Double { Date().timeIntervalSince1970 }

func launcherOnScreen(owner: String, minWidth: Double) -> Bool {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    for w in list {
        guard (w[kCGWindowOwnerName as String] as? String) == owner else { continue }
        let b = w[kCGWindowBounds as String] as? [String: Double] ?? [:]
        if (b["Width"] ?? 0) >= minWidth { return true }
    }
    return false
}

func poll(owner: String, minWidth: Double, timeoutMs: Double, wantVisible: Bool) -> Double? {
    let deadline = now() + timeoutMs / 1000
    while now() < deadline {
        if launcherOnScreen(owner: owner, minWidth: minWidth) == wantVisible { return now() }
        usleep(1000)
    }
    return nil
}

func flagsFrom(_ csv: String) -> CGEventFlags {
    var f = CGEventFlags()
    for p in csv.split(separator: ",") {
        switch p {
        case "alt": f.insert(.maskAlternate)
        case "shift": f.insert(.maskShift)
        case "ctrl": f.insert(.maskControl)
        case "cmd": f.insert(.maskCommand)
        default: break
        }
    }
    return f
}

func post(key: CGKeyCode, flags: CGEventFlags) {
    for down in [true, false] {
        let e = CGEvent(keyboardEventSource: CGEventSource(stateID: .hidSystemState), virtualKey: key, keyDown: down)
        e?.flags = flags
        e?.post(tap: .cghidEventTap)
    }
}

let a = CommandLine.arguments
guard a.count >= 2 else { FileHandle.standardError.write(Data("usage: see header\n".utf8)); exit(2) }
switch a[1] {
case "trusted":
    print(AXIsProcessTrusted())
case "visible", "hidden":
    guard a.count >= 5, let w = Double(a[3]), let t = Double(a[4]) else { exit(2) }
    if let t1 = poll(owner: a[2], minWidth: w, timeoutMs: t, wantVisible: a[1] == "visible") {
        print(a[1], String(format: "%.4f", t1))
    } else { print("timeout"); exit(1) }
case "hotkey":
    guard a.count >= 7, let k = UInt16(a[2]), let w = Double(a[5]), let t = Double(a[6]) else { exit(2) }
    let t0 = now()
    post(key: k, flags: flagsFrom(a[3]))
    print("post", String(format: "%.4f", t0))
    if let t1 = poll(owner: a[4], minWidth: w, timeoutMs: t, wantVisible: true) {
        print("visible", String(format: "%.4f", t1))
        print("latency_ms", String(format: "%.1f", (t1 - t0) * 1000))
    } else { print("timeout"); exit(1) }
case "bounds":
    guard a.count >= 3 else { exit(2) }
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    for w in list where (w[kCGWindowOwnerName as String] as? String) == a[2] {
        let b = w[kCGWindowBounds as String] as? [String: Double] ?? [:]
        print(Int(b["X"] ?? 0), Int(b["Y"] ?? 0), Int(b["Width"] ?? 0), Int(b["Height"] ?? 0), w[kCGWindowLayer as String] as? Int ?? 0)
    }
case "key":
    guard a.count >= 3, let k = UInt16(a[2]) else { exit(2) }
    post(key: k, flags: [])
case "type":
    guard a.count >= 3 else { exit(2) }
    for ch in a[2].utf16 {
        var c = ch
        for down in [true, false] {
            let e = CGEvent(keyboardEventSource: CGEventSource(stateID: .hidSystemState), virtualKey: 0, keyDown: down)
            e?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c)
            e?.post(tap: .cghidEventTap)
        }
        usleep(20000)
    }
default:
    exit(2)
}
