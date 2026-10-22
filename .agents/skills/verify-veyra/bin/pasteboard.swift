import AppKit
import CryptoKit
import Foundation
import Darwin

signal(SIGPIPE, SIG_IGN)

struct PasteboardFailure: Error {}
struct Flavor: Equatable {
    let type: String
    let data: Data
}
typealias Snapshot = [[Flavor]]
let board = NSPasteboard.general

func snapshot() throws -> Snapshot {
    let generation = board.changeCount
    let result = try (board.pasteboardItems ?? []).map { item in
        try item.types.map { type in
            guard let data = item.data(forType: type) else { throw PasteboardFailure() }
            return Flavor(type: type.rawValue, data: data)
        }
    }
    guard board.changeCount == generation else { throw PasteboardFailure() }
    return result
}

func restore(_ saved: Snapshot) throws {
    let items = try saved.map { values in
        let item = NSPasteboardItem()
        for flavor in values {
            guard item.setData(flavor.data, forType: NSPasteboard.PasteboardType(flavor.type)) else {
                throw PasteboardFailure()
            }
        }
        return item
    }
    board.clearContents()
    guard items.isEmpty || board.writeObjects(items) else { throw PasteboardFailure() }
}

func metadata(_ saved: Snapshot, hashes: Bool = false) -> [String: Any] {
    let items: [[String: Any]] = saved.map { values in
        var item: [String: Any] = ["types": values.map { $0.type }]
        if hashes {
            item["sha256"] = Dictionary(uniqueKeysWithValues: values.map { flavor in
                (flavor.type, SHA256.hash(data: flavor.data).map { String(format: "%02x", $0) }.joined())
            })
        }
        return item
    }
    return ["item_count": items.count, "items": items]
}

func emit(_ value: [String: Any]) throws {
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    try FileHandle.standardOutput.write(contentsOf: data + Data([10]))
}

func plantedItems() throws -> Snapshot {
    guard let image = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1, pixelsHigh: 1,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 4, bitsPerPixel: 32) else {
        throw PasteboardFailure()
    }
    image.setColor(NSColor(red: 0.25, green: 0.5, blue: 0.75, alpha: 1), atX: 0, y: 0)
    guard let png = image.representation(using: .png, properties: [:]) else {
        throw PasteboardFailure()
    }
    let text = NSAttributedString(string: "fixture")
    let rtf = try text.data(from: NSRange(location: 0, length: text.length),
        documentAttributes: [.documentType: NSAttributedString.DocumentType.rtf])
    return [
        [Flavor(type: NSPasteboard.PasteboardType.png.rawValue, data: png),
         Flavor(type: "org.veyra.verify-binary", data: Data([0, 1, 255, 0]))],
        [Flavor(type: NSPasteboard.PasteboardType.rtf.rawValue, data: rtf),
         Flavor(type: "public.utf16-external-plain-text", data: "fixture".data(using: .utf16)!),
         Flavor(type: NSPasteboard.PasteboardType.string.rawValue, data: Data("fixture".utf8))],
    ]
}

func run() throws -> Int32 {
    let original = try snapshot()
    var restored = false
    defer {
        if !restored {
            try? restore(original)
            let matches = (try? snapshot()) == original
            try? emit(["phase": "failed", "original_restored": matches, "original": metadata(original)])
        }
    }
    let planted = CommandLine.arguments.contains("--plant")
    let audit = CommandLine.arguments.contains("--audit")
    let plantedSnapshot = planted ? try plantedItems() : nil
    if let plantedSnapshot { try restore(plantedSnapshot) }
    let saved = try snapshot()
    if let plantedSnapshot {
        let savedValues = saved.map { Dictionary(uniqueKeysWithValues: $0.map { ($0.type, $0.data) }) }
        let plantedValues = plantedSnapshot.map { Dictionary(uniqueKeysWithValues: $0.map { ($0.type, $0.data) }) }
        guard savedValues == plantedValues else {
            try emit(["phase": "planted-mismatch", "expected": metadata(plantedSnapshot), "observed": metadata(saved)])
            throw PasteboardFailure()
        }
    }
    if !audit {
        try restore([[Flavor(type: NSPasteboard.PasteboardType.string.rawValue,
            data: Data([81, 27, 91, 50, 48, 49, 126, 27, 91, 50, 48, 48, 126, 0, 3, 7, 127]))]])
    }
    try emit(["phase": "ready", "original": metadata(original), "saved": metadata(saved, hashes: planted)])
    _ = readLine()
    let observed = audit ? try snapshot() : saved
    try restore(saved)
    let after = try snapshot()
    let matches = saved == after && (!audit || observed == saved)
    try restore(original)
    guard try snapshot() == original else { throw PasteboardFailure() }
    restored = true
    try emit(["phase": "restored", "matches": matches, "original_restored": true,
        "before": metadata(saved, hashes: planted), "after": metadata(audit ? observed : after, hashes: planted && matches)])
    return matches ? 0 : 1
}

do {
    exit(try run())
} catch {
    try? emit(["phase": "error", "reason": "pasteboard snapshot or restoration failed"])
    exit(1)
}
