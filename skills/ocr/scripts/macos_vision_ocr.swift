#!/usr/bin/env swift
import Foundation
import Vision
import ImageIO

struct OCRLine: Encodable {
    let text: String
    let confidence: Float
    let boundingBox: [Double]
}

struct OCRResult: Encodable {
    let image: String
    let text: String
    let lines: [OCRLine]
}

func fail(_ message: String, code: Int32 = 1) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(code)
}

let args = Array(CommandLine.arguments.dropFirst())
if args.isEmpty || args.contains("--help") {
    print("Usage: macos_vision_ocr.swift <image-path> [--languages en-US,es-ES] [--fast]")
    exit(args.isEmpty ? 1 : 0)
}

let imagePath = args[0]
var languages = ["en-US"]
var fast = false
var i = 1
while i < args.count {
    switch args[i] {
    case "--languages":
        guard i + 1 < args.count else { fail("--languages requires a comma-separated value") }
        languages = args[i + 1].split(separator: ",").map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
        i += 2
    case "--fast":
        fast = true
        i += 1
    default:
        fail("Unknown argument: \(args[i])")
    }
}

let url = URL(fileURLWithPath: imagePath)
guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
    fail("Could not load image: \(imagePath)")
}

let request = VNRecognizeTextRequest()
request.recognitionLevel = fast ? .fast : .accurate
request.usesLanguageCorrection = true
request.recognitionLanguages = languages

let handler = VNImageRequestHandler(cgImage: image, options: [:])
do {
    try handler.perform([request])
} catch {
    fail("OCR failed: \(error.localizedDescription)")
}

let observations = (request.results ?? [])
    .compactMap { observation -> OCRLine? in
        guard let candidate = observation.topCandidates(1).first else { return nil }
        let box = observation.boundingBox
        return OCRLine(
            text: candidate.string,
            confidence: candidate.confidence,
            boundingBox: [box.origin.x, box.origin.y, box.size.width, box.size.height].map(Double.init)
        )
    }
    .sorted { a, b in
        let ay = a.boundingBox[1]
        let by = b.boundingBox[1]
        if abs(ay - by) > 0.015 { return ay > by }
        return a.boundingBox[0] < b.boundingBox[0]
    }

let text = observations.map { $0.text }.joined(separator: "\n")
let result = OCRResult(image: imagePath, text: text, lines: observations)
let encoder = JSONEncoder()
encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
let data = try encoder.encode(result)
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write("\n".data(using: .utf8)!)
