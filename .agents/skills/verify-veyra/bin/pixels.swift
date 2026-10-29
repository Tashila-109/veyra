import Foundation
import Vision
import ImageIO

let url = URL(fileURLWithPath: CommandLine.arguments[1])
guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { exit(2) }
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = false
try VNImageRequestHandler(cgImage: image).perform([request])
let rows: [[String: Any]] = (request.results ?? []).compactMap { observation in
    guard let candidate = observation.topCandidates(1).first else { return nil }
    let box = observation.boundingBox
    let text = candidate.string
    let words: [[String: Any]] = try! NSRegularExpression(pattern: "\\S+")
        .matches(in: text, range: NSRange(text.startIndex..., in: text)).map { match in
            let range = Range(match.range, in: text)!
            let box = try! candidate.boundingBox(for: range)!.boundingBox
            return ["text": String(text[range]), "box": [box.minX * Double(image.width),
                (1 - box.maxY) * Double(image.height), box.maxX * Double(image.width),
                (1 - box.minY) * Double(image.height)]]
        }
    return ["text": candidate.string, "confidence": candidate.confidence,
        "words": words,
        "box": [box.minX * Double(image.width), (1 - box.maxY) * Double(image.height),
                box.maxX * Double(image.width), (1 - box.minY) * Double(image.height)]]
}
FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: rows, options: [.sortedKeys]))
