// Draws the app icon: a ticket stock in the preview's own colours, tilted on a
// dark tile, with a perforated stub and a barcode on it.
//
//   swift scripts/make-icon.swift bundle/icon-1024.png
//
// The PNG is committed so a build needs nothing but sips and iconutil; rerun
// this after changing it.
import AppKit

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon-1024.png"
let s = 1024

func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: alpha)
}

// The same values as `Palette` in src/preview.rs.
let stock = rgb(0xfaf9f4)
let ink = rgb(0x18181c)
let accent = rgb(0xe0533d)

// An explicit 1024-pixel bitmap: NSImage.lockFocus would follow the screen's
// scale and write 2048 on a Retina Mac.
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: s, pixelsHigh: s, bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
    bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let ctx = NSGraphicsContext.current!.cgContext
let space = CGColorSpace(name: CGColorSpace.sRGB)!

// The tile, on the macOS icon grid: 824 square in 1024, with a soft shadow.
let body = CGPath(
    roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824),
    cornerWidth: 186, cornerHeight: 186, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: rgb(0x000000, 0.35))
ctx.addPath(body)
ctx.setFillColor(rgb(0x101116))
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(body)
ctx.clip()
let ground = CGGradient(
    colorsSpace: space, colors: [rgb(0x26272e), rgb(0x0c0d11)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(
    ground, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
ctx.restoreGState()

// The ticket, in its own coordinates: centred on the origin, the stub on the
// right behind a perforation, with a notch where the perforation meets each edge.
let w: CGFloat = 660
let h: CGFloat = 340
let ticket = CGRect(x: -w / 2, y: -h / 2, width: w, height: h)
let perf = ticket.minX + w * 0.69
let notch: CGFloat = 34

ctx.saveGState()
ctx.translateBy(x: 512, y: 500)
ctx.rotate(by: 14 * .pi / 180)

ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 30, color: rgb(0x000000, 0.5))
ctx.beginTransparencyLayer(auxiliaryInfo: nil)

ctx.saveGState()
ctx.addRect(ticket.insetBy(dx: -50, dy: -50))
ctx.addEllipse(in: CGRect(x: perf - notch, y: ticket.maxY - notch, width: notch * 2, height: notch * 2))
ctx.addEllipse(in: CGRect(x: perf - notch, y: ticket.minY - notch, width: notch * 2, height: notch * 2))
ctx.clip(using: .evenOdd)
ctx.addPath(CGPath(roundedRect: ticket, cornerWidth: 26, cornerHeight: 26, transform: nil))
ctx.setFillColor(stock)
ctx.fillPath()
ctx.restoreGState()

// Perforation.
ctx.setFillColor(rgb(0x18181c, 0.28))
var y = ticket.minY + notch + 22
while y < ticket.maxY - notch - 22 {
    ctx.fillEllipse(in: CGRect(x: perf - 5, y: y - 5, width: 10, height: 10))
    y += 26
}

// The face: a headline in the accent, then lines of type in ink.
func bar(_ x: CGFloat, _ y: CGFloat, _ width: CGFloat, _ height: CGFloat, _ color: CGColor) {
    ctx.addPath(CGPath(
        roundedRect: CGRect(x: x, y: y, width: width, height: height),
        cornerWidth: height / 2, cornerHeight: height / 2, transform: nil))
    ctx.setFillColor(color)
    ctx.fillPath()
}
let left = ticket.minX + 52
bar(left, 62, 300, 50, accent)
bar(left, 4, 220, 24, ink)
bar(left, -40, 160, 24, ink)
bar(left, -112, 250, 34, ink)

// A barcode across the stub.
let widths: [CGFloat] = [12, 6, 6, 16, 6, 12, 6, 16, 6]
let gap: CGFloat = 8
let total = widths.reduce(0, +) + CGFloat(widths.count - 1) * gap
var x = (perf + ticket.maxX) / 2 - total / 2
ctx.setFillColor(ink)
for bw in widths {
    ctx.fill(CGRect(x: x, y: -112, width: bw, height: 224))
    x += bw + gap
}

ctx.endTransparencyLayer()
ctx.restoreGState()

NSGraphicsContext.current = nil
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
