// Reference floor for "idle RSS": a do-nothing accessory AppKit process (no Dock icon, no visible
// window) that links AppKit + Metal + QuartzCore like a wgpu/winit app does. Prints its own RSS and
// footprint-relevant numbers after a settle time, so docs/PERF.md can say how much of the launcher's
// RSS is "what any AppKit+Metal process pays" and how much is Ashdash.
// With "show" it also orders a 700x63 transparent borderless Metal window front and presents one cleared
// frame (SHOWS A WINDOW: hold the desktop lock), to see what merely showing such a window costs.
//   swiftc -O perf-floor.swift -o /tmp/perf-floor && /tmp/perf-floor [seconds] [metal] [show] [effect]
import AppKit
import Metal
import QuartzCore

let secs = Double(CommandLine.arguments.dropFirst().first ?? "6") ?? 6
let showWindow = CommandLine.arguments.contains("show")
let withEffect = CommandLine.arguments.contains("effect")   // NSVisualEffectView behind the content, as gui/mac.rs does
let withMetal = CommandLine.arguments.contains("metal") || showWindow
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let win = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 60), styleMask: [.borderless], backing: .buffered, defer: false)
win.isOpaque = false
win.backgroundColor = .clear
if withMetal {
    // what wgpu does at startup: create the device and a command queue, attach a CAMetalLayer.
    if let dev = MTLCreateSystemDefaultDevice() {
        _ = dev.makeCommandQueue()
        let layer = CAMetalLayer()
        layer.device = dev
        win.contentView?.wantsLayer = true
        win.contentView?.layer = layer
    }
}
if showWindow, let layer = win.contentView?.layer as? CAMetalLayer {
    layer.pixelFormat = .bgra8Unorm
    layer.drawableSize = CGSize(width: 1400, height: 126)
    layer.isOpaque = false
    win.hasShadow = true
    win.level = .statusBar
    if withEffect, let content = win.contentView {
        let v = NSVisualEffectView(frame: content.bounds)
        v.autoresizingMask = [.width, .height]
        v.blendingMode = .behindWindow
        v.state = .active
        v.material = .hudWindow
        v.appearance = NSAppearance(named: .vibrantDark)
        v.wantsLayer = true
        v.layer?.cornerRadius = 12
        v.layer?.masksToBounds = true
        content.addSubview(v, positioned: .below, relativeTo: nil)
    }
    win.makeKeyAndOrderFront(nil)
    if let drawable = layer.nextDrawable(), let dev = layer.device, let q = dev.makeCommandQueue(), let cb = q.makeCommandBuffer() {
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.1, green: 0.1, blue: 0.1, alpha: 0.9)
        pass.colorAttachments[0].storeAction = .store
        cb.makeRenderCommandEncoder(descriptor: pass)?.endEncoding()
        cb.present(drawable)
        cb.commit()
    }
}
// without "show" the window is never ordered front: it stays hidden
DispatchQueue.main.asyncAfter(deadline: .now() + secs) {
    var ti = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
    let kr = withUnsafeMutablePointer(to: &ti) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count) }
    }
    if kr == KERN_SUCCESS {
        print("metal=\(withMetal) show=\(showWindow) effect=\(withEffect) phys_footprint_MB=\(ti.phys_footprint / 1048576) resident_MB=\(ti.resident_size / 1048576)")
    }
    exit(0)
}
app.run()
