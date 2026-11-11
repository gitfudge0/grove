// Composite the selected installer art and a vector arrow at Finder's 720×440 size.
// Render in logical pixels: Finder displays PNG backgrounds at pixel size.
import AppKit

let source = URL(fileURLWithPath: CommandLine.arguments[1])
let output = URL(fileURLWithPath: CommandLine.arguments[2])
guard let artwork = NSImage(contentsOf: source),
      let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 720, pixelsHigh: 440,
          bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
          colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else {
    fatalError("Could not load installer artwork or create its bitmap")
}
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
NSGraphicsContext.current?.imageInterpolation = .high
artwork.draw(in: NSRect(x: 0, y: 0, width: 720, height: 440))
let arrow = NSBezierPath()
// Screen coordinates: M307,218 Q360,187 413,218.
arrow.move(to: NSPoint(x: 307, y: 222))
arrow.curve(to: NSPoint(x: 413, y: 222),
    controlPoint1: NSPoint(x: 342.33, y: 242.67),
    controlPoint2: NSPoint(x: 377.67, y: 242.67))
arrow.move(to: NSPoint(x: 402, y: 232))
arrow.line(to: NSPoint(x: 413, y: 222))
arrow.line(to: NSPoint(x: 399, y: 218))
arrow.lineWidth = 2.5
arrow.lineCapStyle = .round
arrow.lineJoinStyle = .round
NSColor(srgbRed: 0.33, green: 0.45, blue: 0.28, alpha: 1).setStroke()
arrow.stroke()
NSGraphicsContext.restoreGraphicsState()
guard let png = bitmap.representation(using: .png, properties: [:]) else {
    fatalError("Could not encode installer background")
}
try png.write(to: output)
