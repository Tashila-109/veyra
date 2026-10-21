#!/usr/bin/env swift
import AppKit
import CoreGraphics
import Foundation
let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 2, let pid = Int32(args[0]), let id = UInt32(args[1]) else { exit(2) }
func fail(_ text: String) -> Never {
    fputs(text + ". Veyra must stay frontmost during captures.\n", stderr)
    exit(1)
}
guard NSRunningApplication(processIdentifier: pid) != nil else { fail("owned app is gone") }
let end = Date().addingTimeInterval(2)
while NSWorkspace.shared.frontmostApplication?.processIdentifier != pid && Date() < end {
    RunLoop.current.run(until: Date().addingTimeInterval(0.05))
}
guard let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]],
      let window = windows.first(where: {
          ($0[kCGWindowNumber as String] as? UInt32) == id &&
          ($0[kCGWindowOwnerPID as String] as? Int32) == pid &&
          ($0[kCGWindowLayer as String] as? Int) == 0
      }), let bounds = window[kCGWindowBounds as String] as? [String: NSNumber],
      let x = bounds["X"], let y = bounds["Y"], let width = bounds["Width"], let height = bounds["Height"]
else { fail("owned window is off-screen") }
let frame = CGRect(x: x.doubleValue, y: y.doubleValue, width: width.doubleValue, height: height.doubleValue)
var count: UInt32 = 0
CGGetActiveDisplayList(0, nil, &count)
var displays = [CGDirectDisplayID](repeating: 0, count: Int(count))
CGGetActiveDisplayList(count, &displays, &count)
guard displays.contains(where: { frame.intersects(CGDisplayBounds($0)) })
else { fail("owned window is outside the active displays") }
guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid
else { fail("owned window is covered by another application") }
