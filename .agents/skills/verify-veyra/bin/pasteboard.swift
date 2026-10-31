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
var board: NSPasteboard { NSPasteboard.general }

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

// The text-comparison lane owns a snapshot before its caller performs Copy.
// Its stdout carries only the result; the snapshot never leaves memory.
protocol TextComparisonBoard {
    func capture() throws -> Snapshot
    func restoreSnapshot(_ saved: Snapshot) throws
    func currentText() -> String?
}
struct SystemComparisonBoard: TextComparisonBoard {
    func capture() throws -> Snapshot { try snapshot() }
    func restoreSnapshot(_ saved: Snapshot) throws { try restore(saved) }
    func currentText() -> String? { board.string(forType: .string) }
}
func compareInSnapshotPath<B: TextComparisonBoard>(
    _ access: B, expected: String, active: Bool, readyAndWait: () throws -> Void
) throws -> Bool {
    guard active else { return false }
    let saved = try access.capture()
    var restored = false
    defer { if !restored { try? access.restoreSnapshot(saved) } }
    try readyAndWait()
    let matches = access.currentText().map { Array($0.utf8) } == Array(expected.utf8)
    try access.restoreSnapshot(saved)
    guard try access.capture() == saved else { throw PasteboardFailure() }
    restored = true
    return matches
}
func comparisonPaths(_ args: [String]) -> (String, String)? {
    guard args.count == 5, args[0] == "--audit", args[1] == "--expect-text",
        args[3] == "--ready-file", !args[2].isEmpty, !args[4].isEmpty,
        args[2] != args[4] else { return nil }
    return (args[2], args[4])
}
func compareText(_ args: [String]) -> Bool {
    // Refuse before creating a system-board adapter or reading the pasteboard.
    guard let (expectedPath, readyPath) = comparisonPaths(args),
        !FileManager.default.fileExists(atPath: readyPath),
        let data = FileManager.default.contents(atPath: expectedPath),
        let expected = String(data: data, encoding: .utf8) else { return false }
    do {
        return try compareInSnapshotPath(SystemComparisonBoard(), expected: expected, active: true) {
            guard FileManager.default.createFile(atPath: readyPath, contents: Data("ready\n".utf8),
                attributes: [.posixPermissions: 0o600]) else { throw PasteboardFailure() }
            _ = readLine()
        }
    } catch { return false }
}
func comparisonSelfTest() -> Bool {
    final class StubBoard: TextComparisonBoard {
        var value: String? = "human"
        var captures = 0
        func capture() throws -> Snapshot {
            captures += 1
            return value.map { [[Flavor(type: "stub", data: Data($0.utf8))]] } ?? []
        }
        func restoreSnapshot(_ saved: Snapshot) throws {
            value = saved.first?.first.flatMap { String(data: $0.data, encoding: .utf8) }
        }
        func currentText() -> String? { value }
    }
    do {
        let stub = StubBoard()
        guard try !compareInSnapshotPath(stub, expected: "fixture", active: false,
            readyAndWait: { throw PasteboardFailure() }), stub.captures == 0,
            comparisonPaths(["--expect-text", "fixture"]) == nil else { return false }
        for value: String? in ["fixture", "other", nil] {
            let matches = try compareInSnapshotPath(stub, expected: "fixture", active: true) {
                stub.value = value
            }
            guard matches == (value == "fixture"), stub.value == "human" else { return false }
        }
        let canonical = try compareInSnapshotPath(stub, expected: "é", active: true) {
            stub.value = "e\u{301}"
        }
        guard !canonical, stub.value == "human" else { return false }
        do {
            _ = try compareInSnapshotPath(stub, expected: "fixture", active: true) {
                stub.value = "changed"
                throw PasteboardFailure()
            }
            return false
        } catch { guard stub.value == "human" else { return false } }
        return comparisonPaths(["--audit", "--expect-text", "expected", "--ready-file", "ready"]) != nil
    } catch { return false }
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

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments == ["--self-test"] {
    let matches = comparisonSelfTest()
    print(matches ? "match" : "no-match")
    exit(matches ? 0 : 1)
}
if arguments.contains("--expect-text") {
    let matches = compareText(arguments)
    print(matches ? "match" : "no-match")
    exit(matches ? 0 : 1)
}
do {
    exit(try run())
} catch {
    try? emit(["phase": "error", "reason": "pasteboard snapshot or restoration failed"])
    exit(1)
}
