#!/usr/bin/env swift
import CoreGraphics
import Foundation

let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 1 || (args.count == 2 && args[1] == "--bounds"),
      let pid = Int(args[0]) else {
    fputs("usage: winid.swift PID [--bounds]\n", stderr)
    exit(2)
}
guard let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] else {
    fputs("cannot read on-screen windows\n", stderr)
    exit(1)
}
guard let window = windows.first(where: {
    ($0[kCGWindowOwnerPID as String] as? Int) == pid &&
    ($0[kCGWindowLayer as String] as? Int) == 0
}), let number = window[kCGWindowNumber as String] as? Int else {
    fputs("no on-screen window for pid \(pid)\n", stderr)
    exit(1)
}
if args.count == 2 {
    guard let bounds = window[kCGWindowBounds as String] as? [String: Any],
          let originX = bounds["X"] as? NSNumber,
          let originY = bounds["Y"] as? NSNumber,
          let width = bounds["Width"] as? NSNumber,
          let height = bounds["Height"] as? NSNumber else {
        fputs("window bounds unavailable\n", stderr)
        exit(1)
    }
    print("\(originX.intValue),\(originY.intValue),\(width.intValue),\(height.intValue)")
} else {
    print(number)
}
