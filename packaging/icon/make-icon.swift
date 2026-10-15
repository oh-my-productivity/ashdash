// Draws the Ashdash app icon with CoreGraphics and writes an .iconset directory.
//
// The artwork is original: a rounded-square gradient tile with a magnifying glass whose
// lens holds a command-prompt chevron. Everything is vector code, so every size is
// rendered natively (no downscaling blur) and no binary art lives in the repository.
//
// Usage: swift make-icon.swift <output.iconset>
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let canvas: CGFloat = 1024  // all coordinates below are in a 1024x1024, y-up space

func rgb(_ r: Int, _ g: Int, _ b: Int, _ a: CGFloat = 1) -> CGColor {
    CGColor(
        colorSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
        components: [CGFloat(r) / 255, CGFloat(g) / 255, CGFloat(b) / 255, a])!
}

func draw(_ ctx: CGContext) {
    // Apple's icon grid: an 824-pt tile centred in the 1024 canvas, soft shadow below.
    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    let radius: CGFloat = 185
    let tilePath = CGPath(roundedRect: tile, cornerWidth: radius, cornerHeight: radius, transform: nil)

    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 28, color: rgb(0, 0, 0, 0.35))
    ctx.setFillColor(rgb(30, 27, 75))
    ctx.addPath(tilePath)
    ctx.fillPath()
    ctx.restoreGState()

    // Background gradient, light indigo (top-left) to deep navy (bottom-right).
    ctx.saveGState()
    ctx.addPath(tilePath)
    ctx.clip()
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let gradient = CGGradient(
        colorsSpace: space,
        colors: [rgb(99, 102, 241), rgb(67, 56, 202), rgb(30, 27, 75)] as CFArray,
        locations: [0, 0.55, 1])!
    ctx.drawLinearGradient(
        gradient, start: CGPoint(x: tile.minX, y: tile.maxY), end: CGPoint(x: tile.maxX, y: tile.minY), options: [])
    // Soft highlight across the top for depth.
    let sheen = CGGradient(
        colorsSpace: space, colors: [rgb(255, 255, 255, 0.22), rgb(255, 255, 255, 0)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(
        sheen, start: CGPoint(x: 0, y: tile.maxY), end: CGPoint(x: 0, y: tile.midY), options: [])
    ctx.restoreGState()

    // Magnifying glass.
    let center = CGPoint(x: 458, y: 574)
    let lensRadius: CGFloat = 215
    let ringWidth: CGFloat = 66
    let angle: CGFloat = -.pi / 4  // handle points to the lower right
    let handleStart = CGPoint(
        x: center.x + cos(angle) * (lensRadius + ringWidth / 2 - 6),
        y: center.y + sin(angle) * (lensRadius + ringWidth / 2 - 6))
    let handleEnd = CGPoint(x: center.x + cos(angle) * 395, y: center.y + sin(angle) * 395)

    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 18, color: rgb(10, 8, 40, 0.45))
    ctx.setLineCap(.round)
    ctx.setStrokeColor(rgb(255, 255, 255))
    ctx.setLineWidth(ringWidth)
    ctx.addArc(center: center, radius: lensRadius, startAngle: 0, endAngle: 2 * .pi, clockwise: false)
    ctx.strokePath()
    ctx.setLineWidth(ringWidth + 22)
    ctx.move(to: handleStart)
    ctx.addLine(to: handleEnd)
    ctx.strokePath()
    ctx.restoreGState()

    // Lens glass: faint tint so the chevron reads as "inside" the lens.
    ctx.setFillColor(rgb(255, 255, 255, 0.10))
    ctx.addArc(center: center, radius: lensRadius - ringWidth / 2, startAngle: 0, endAngle: 2 * .pi, clockwise: false)
    ctx.fillPath()

    // Prompt chevron ">" and cursor bar inside the lens.
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.setStrokeColor(rgb(103, 232, 249))
    ctx.setLineWidth(48)
    ctx.move(to: CGPoint(x: center.x - 82, y: center.y + 78))
    ctx.addLine(to: CGPoint(x: center.x + 6, y: center.y))
    ctx.addLine(to: CGPoint(x: center.x - 82, y: center.y - 78))
    ctx.strokePath()
    ctx.setLineWidth(40)
    ctx.move(to: CGPoint(x: center.x + 44, y: center.y - 80))
    ctx.addLine(to: CGPoint(x: center.x + 112, y: center.y - 80))
    ctx.strokePath()
}

func render(pixels: Int) -> CGImage {
    let ctx = CGContext(
        data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    ctx.scaleBy(x: CGFloat(pixels) / canvas, y: CGFloat(pixels) / canvas)
    draw(ctx)
    return ctx.makeImage()!
}

func writePNG(_ image: CGImage, to url: URL) {
    guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        FileHandle.standardError.write(Data("cannot create \(url.path)\n".utf8))
        exit(1)
    }
    CGImageDestinationAddImage(dest, image, nil)
    if !CGImageDestinationFinalize(dest) {
        FileHandle.standardError.write(Data("cannot write \(url.path)\n".utf8))
        exit(1)
    }
}

guard CommandLine.arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: make-icon.swift <output.iconset>\n".utf8))
    exit(2)
}
let out = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

// (file name, pixel size) pairs required by `iconutil`.
let variants: [(String, Int)] = [
    ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024),
]
var cache: [Int: CGImage] = [:]
for (name, pixels) in variants {
    let image = cache[pixels] ?? render(pixels: pixels)
    cache[pixels] = image
    writePNG(image, to: out.appendingPathComponent(name))
}
