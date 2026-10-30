import AppKit
import CoreGraphics
let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 2, let pid = Int(args[0]), let number = UInt32(args[1]),
      let rows = CGWindowListCopyWindowInfo(.optionIncludingWindow, number) as? [[String: Any]],
      let row = rows.first(where: { ($0[kCGWindowNumber as String] as? Int) == Int(number) && ($0[kCGWindowOwnerPID as String] as? Int) == pid }),
      let bounds = row[kCGWindowBounds as String] as? [String: Any],
      let x = bounds["X"] as? NSNumber, let y = bounds["Y"] as? NSNumber,
      let w = bounds["Width"] as? NSNumber, let h = bounds["Height"] as? NSNumber,
      let primary = NSScreen.screens.first else { exit(1) }
let point = NSPoint(x: x.doubleValue + w.doubleValue / 2, y: primary.frame.maxY - y.doubleValue - h.doubleValue / 2)
let screens = NSScreen.screens.filter { $0.frame.contains(point) }
guard screens.count == 1 else { exit(1) }
let frame = screens[0].visibleFrame
let value = ["visible_frame": [frame.minX, primary.frame.maxY - frame.maxY, frame.width, frame.height]]
let data = try JSONSerialization.data(withJSONObject: value)
print(String(data: data, encoding: .utf8)!)
