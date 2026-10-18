// Control for the "+185 MB footprint when the launcher window is first shown" (docs/PERF.md, W4-PERF2):
// a bare AppKit window with a CAMetalLayer and ONE clear-colour render pass, no wgpu/egui/Ashdash code.
// Prints the process footprint after each step. Build and run:  swiftc -O scripts/qa/perf-metal-control.swift -o /tmp/ctl && /tmp/ctl
// (it opens a 640x400 borderless window for about a second: hold the desktop lock first).
import AppKit
import Metal
import QuartzCore
func fp() -> String {
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/bin/footprint"); p.arguments = ["-p", "\(getpid())"]
    let pipe = Pipe(); p.standardOutput = pipe; try? p.run(); p.waitUntilExit()
    let s = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    return s.split(separator: "\n").first(where: { $0.contains("Footprint:") }).map(String.init) ?? "?"
}
let mode = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "clear"
let app = NSApplication.shared; app.setActivationPolicy(.accessory)
print("start", fp())
let dev = MTLCreateSystemDefaultDevice()!
let q = dev.makeCommandQueue()!
print("device+queue", fp())
let win = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 640, height: 400), styleMask: [.borderless], backing: .buffered, defer: false)
let layer = CAMetalLayer(); layer.device = dev; layer.pixelFormat = .bgra8Unorm; layer.drawableSize = CGSize(width: 1280, height: 800)
let v = NSView(frame: NSRect(x: 0, y: 0, width: 640, height: 400)); v.wantsLayer = true; v.layer = layer; win.contentView = v
win.orderFrontRegardless()
print("window", fp())
func frame() {
    guard let d = layer.nextDrawable(), let cb = q.makeCommandBuffer() else { return }
    let rp = MTLRenderPassDescriptor(); rp.colorAttachments[0].texture = d.texture; rp.colorAttachments[0].loadAction = .clear
    rp.colorAttachments[0].clearColor = MTLClearColorMake(0.1, 0.1, 0.1, 1); rp.colorAttachments[0].storeAction = .store
    let e = cb.makeRenderCommandEncoder(descriptor: rp)!; e.endEncoding(); cb.present(d); cb.commit()
}
frame(); RunLoop.current.run(until: Date().addingTimeInterval(0.5))
print("after first frame", fp())
frame(); RunLoop.current.run(until: Date().addingTimeInterval(0.5))
print("after 2nd frame", fp())
