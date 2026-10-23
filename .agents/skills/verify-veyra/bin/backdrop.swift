#!/usr/bin/env swift
import AppKit
import CoreGraphics
import Dispatch
import Foundation

struct BackdropOptions {
    let coarse: Bool
    let inverted: Bool
}

let arguments = Array(CommandLine.arguments.dropFirst())
let usage = "usage: backdrop [--coarse] [--invert]"
if arguments == ["--help"] {
    print(usage)
    exit(0)
}
guard arguments.allSatisfy({ $0 == "--coarse" || $0 == "--invert" }) else {
    fputs(usage + "\n", stderr)
    exit(2)
}
let options = BackdropOptions(coarse: arguments.contains("--coarse"), inverted: arguments.contains("--invert"))

final class BackdropView: NSView {
    override var isOpaque: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let background = options.inverted ? NSColor.black : NSColor.white
        let foreground = options.inverted ? NSColor.white : NSColor.black
        let blockWidth: CGFloat = options.coarse ? 400 : 32
        background.setFill()
        bounds.fill()
        for x in stride(from: CGFloat.zero, to: bounds.width, by: blockWidth * 2) {
            foreground.setFill()
            NSRect(x: x, y: 0, width: blockWidth, height: bounds.height).fill()
        }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: options.coarse ? 160 : 20, weight: .medium),
            .foregroundColor: foreground
        ]
        if options.coarse {
            let text = "VEYRA 0123456789" as NSString
            let size = text.size(withAttributes: attributes)
            let y = (bounds.height - size.height) / 2
            background.setFill()
            NSRect(x: 0, y: y - 16, width: bounds.width, height: size.height + 32).fill()
            text.draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: y), withAttributes: attributes)
            return
        }
        let text = "Veyra backdrop  ABCDEFGH  0123456789" as NSString
        let step = text.size(withAttributes: attributes).width + 32
        for y in stride(from: CGFloat(24), to: bounds.height, by: 96) {
            background.setFill()
            NSRect(x: 0, y: y, width: bounds.width, height: 36).fill()
            for x in stride(from: CGFloat(16), to: bounds.width, by: step) {
                text.draw(at: NSPoint(x: x, y: y + 6), withAttributes: attributes)
            }
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let screens = NSScreen.screens
guard !screens.isEmpty else {
    fputs("screens unavailable\n", stderr)
    exit(1)
}
let windows = screens.map { screen in
    let window = NSWindow(
        contentRect: screen.frame,
        styleMask: .borderless,
        backing: .buffered,
        defer: false
    )
    window.level = .normal
    window.isOpaque = true
    window.backgroundColor = .white
    window.hasShadow = false
    window.ignoresMouseEvents = true
    window.isReleasedWhenClosed = false
    window.contentView = BackdropView(frame: NSRect(origin: .zero, size: screen.frame.size))
    window.setFrame(screen.frame, display: true)
    return window
}

signal(SIGTERM, SIG_IGN)
let termination = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
termination.setEventHandler { app.terminate(nil) }
termination.resume()

for window in windows { window.orderFrontRegardless() }
let pid = ProcessInfo.processInfo.processIdentifier
if let pidFile = Bundle.main.object(forInfoDictionaryKey: "VeyraBackdropPidFile") as? String {
    do {
        try "\(pid)\n".write(toFile: pidFile, atomically: true, encoding: .utf8)
    } catch {
        fputs("backdrop pid file unavailable\n", stderr)
        exit(1)
    }
}
func windowRecords() -> [[String: Any]]? {
    guard let listed = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] else {
        return nil
    }
    var records: [[String: Any]] = []
    for (screen, window) in zip(screens, windows) {
        guard let entry = listed.first(where: {
            ($0[kCGWindowNumber as String] as? NSNumber)?.intValue == window.windowNumber &&
            ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid &&
            ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0
        }), let bounds = entry[kCGWindowBounds as String] as? [String: NSNumber],
            let x = bounds["X"], let y = bounds["Y"],
            let width = bounds["Width"], let height = bounds["Height"],
            width.doubleValue > 0, height.doubleValue > 0 else {
            return nil
        }
        records.append([
            "pid": pid,
            "window": window.windowNumber,
            "coarse": options.coarse,
            "inverted": options.inverted,
            "block_width": options.coarse ? 400 : 32,
            "text_size": options.coarse ? 160 : 20,
            "screen_frame": [screen.frame.minX, screen.frame.minY, screen.frame.width, screen.frame.height],
            "bounds": [x.doubleValue, y.doubleValue, width.doubleValue, height.doubleValue]
        ])
    }
    return records
}
let deadline = Date().addingTimeInterval(5)
let readiness = Timer(timeInterval: 0.05, repeats: true) { timer in
    if let records = windowRecords() {
        timer.invalidate()
        for record in records {
            guard let data = try? JSONSerialization.data(withJSONObject: record, options: .sortedKeys) else {
                fputs("backdrop window record unavailable\n", stderr)
                exit(1)
            }
            print("backdrop window \(String(decoding: data, as: UTF8.self))")
        }
        print("backdrop ready pid=\(pid) windows=\(windows.count)")
        fflush(stdout)
    } else if Date() >= deadline {
        timer.invalidate()
        fputs("backdrop windows unavailable after five seconds\n", stderr)
        exit(1)
    }
}
RunLoop.main.add(readiness, forMode: .common)
app.run()
