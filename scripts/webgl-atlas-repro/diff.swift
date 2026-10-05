// Pixel-compare two PNGs of equal size. Prints the number of differing pixels
// and writes a diff image (differing pixels red over a dimmed copy of `a`).
// Usage: swift diff.swift <a.png> <b.png> <diff.png>
import AppKit

func rgba(_ path: String) -> (data: [UInt8], width: Int, height: Int) {
  guard let image = NSImage(contentsOfFile: path),
        let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    print("cannot read \(path)")
    exit(1)
  }
  let width = cg.width, height = cg.height
  var data = [UInt8](repeating: 0, count: width * height * 4)
  let ctx = CGContext(data: &data, width: width, height: height, bitsPerComponent: 8,
                      bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  ctx.draw(cg, in: CGRect(x: 0, y: 0, width: width, height: height))
  return (data, width, height)
}

let args = CommandLine.arguments
let a = rgba(args[1]), b = rgba(args[2])
guard a.width == b.width, a.height == b.height else {
  print("size mismatch \(a.width)x\(a.height) vs \(b.width)x\(b.height)")
  exit(1)
}
var out = [UInt8](repeating: 0, count: a.data.count)
var differing = 0
var rows = Set<Int>()
for i in stride(from: 0, to: a.data.count, by: 4) {
  let delta = (0..<3).map { abs(Int(a.data[i + $0]) - Int(b.data[i + $0])) }.max()!
  if delta > 8 {
    differing += 1
    rows.insert((i / 4) / a.width)
    out[i] = 255; out[i + 1] = 0; out[i + 2] = 0; out[i + 3] = 255
  } else {
    out[i] = a.data[i] / 3; out[i + 1] = a.data[i + 1] / 3; out[i + 2] = a.data[i + 2] / 3; out[i + 3] = 255
  }
}
let ctx = CGContext(data: &out, width: a.width, height: a.height, bitsPerComponent: 8,
                    bytesPerRow: a.width * 4, space: CGColorSpaceCreateDeviceRGB(),
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: args[3]))
print("differing=\(differing) pixelRows=\(rows.count)")
