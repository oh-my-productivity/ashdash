// OCR a PNG with Vision and print the recognized lines (top to bottom). Used by the QA scripts to read
// what the launcher drew, since egui exposes no accessibility tree.
import Foundation
import Vision
import AppKit

guard CommandLine.arguments.count > 1, let img = NSImage(contentsOfFile: CommandLine.arguments[1]),
      let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write(Data("usage: qa-ocr file.png\n".utf8)); exit(2)
}
let req = VNRecognizeTextRequest()
req.recognitionLevel = .accurate
req.usesLanguageCorrection = false
let handler = VNImageRequestHandler(cgImage: cg, options: [:])
try handler.perform([req])
let obs = (req.results ?? []).sorted { $0.boundingBox.midY > $1.boundingBox.midY }
for o in obs { if let t = o.topCandidates(1).first { print(t.string) } }
