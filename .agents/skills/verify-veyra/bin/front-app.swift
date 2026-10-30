import AppKit
import Foundation
guard CommandLine.arguments.count == 2, let pid = Int32(CommandLine.arguments[1]) else { exit(2) }
let value: [String: Any] = ["owned_pid": pid,
    "frontmost_is_owned": NSWorkspace.shared.frontmostApplication?.processIdentifier == pid]
print(String(data: try JSONSerialization.data(withJSONObject: value), encoding: .utf8)!)
