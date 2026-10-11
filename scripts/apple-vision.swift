import Foundation
import Vision
import ImageIO
import CoreML

func run() throws {
    if CommandLine.arguments.count == 2 && CommandLine.arguments[1] == "--languages" {
        let request = VNRecognizeTextRequest()
        print(String(data: try JSONSerialization.data(withJSONObject: request.supportedRecognitionLanguages()), encoding: .utf8)!)
        return
    }
    guard CommandLine.arguments.count >= 2 else { throw NSError(domain: "usage: apple-vision IMAGE [language+language]", code: 1) }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = true
    if #available(macOS 14.0, *) {
        for (stage, devices) in try request.supportedComputeStageDevices {
            if let cpu = devices.first(where: { if case .cpu = $0 { return true }; return false }) {
                request.setComputeDevice(cpu, for: stage)
            }
        }
    } else { request.usesCPUOnly = true }
    if CommandLine.arguments.count >= 3 {
        let requested = CommandLine.arguments[2].split(separator: "+").map { String($0) == "eng" ? "en-US" : String($0) }
        let supported = try request.supportedRecognitionLanguages()
        guard requested.allSatisfy({ supported.contains($0) }) else { throw NSError(domain: "unsupported OCR language; query --languages", code: 2) }
        request.recognitionLanguages = requested
    }
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL, nil), let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { throw NSError(domain: "corrupt image", code: 3) }
    let handler = VNImageRequestHandler(cgImage: image)
    try handler.perform([request])
    let rows: [[String: Any]] = (request.results ?? []).compactMap { observation in
        guard let text = observation.topCandidates(1).first else { return nil }
        let b = observation.boundingBox
        return ["text": text.string, "score_uncalibrated": text.confidence,
                "bounds": ["x": b.minX, "y": 1-b.maxY, "width": b.width, "height": b.height, "coordinate_space": "normalized_image_top_left"]]
    }
    print(String(data: try JSONSerialization.data(withJSONObject: rows), encoding: .utf8)!)
}
do { try run() } catch { fputs("\(error)\n", stderr); exit(1) }
