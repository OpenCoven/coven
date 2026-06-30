---
name: ocr
description: Extract text from screenshots, scanned documents, image files, and PDFs. Use when a user asks to OCR, read text from an image/screenshot/scan, transcribe visible text, extract text from a scanned PDF, compare OCR text, or convert image/PDF text into Markdown/JSON/plain text.
---

# OCR

Use this skill to extract text from local screenshots, images, scans, and PDFs with a repeatable workflow.

## Quick start

Prefer the bundled helper when the user needs exact-ish text extraction from a local file:

```bash
python3 ~/.openclaw/workspace/skills/ocr/scripts/ocr.py <path> --format text
```

Useful variants:

```bash
# Structured output with per-line confidence/bounding boxes
python3 ~/.openclaw/workspace/skills/ocr/scripts/ocr.py screenshot.png --format json

# Markdown output from a scanned PDF, OCRing up to the first 5 pages
python3 ~/.openclaw/workspace/skills/ocr/scripts/ocr.py scan.pdf --force-ocr --max-pages 5 --format markdown

# Multiple languages for macOS Vision OCR
python3 ~/.openclaw/workspace/skills/ocr/scripts/ocr.py receipt.jpg --languages en-US,es-ES --format text
```

## Workflow

1. Locate the file. If the image/PDF is attached in chat, use the local attachment path the runtime provides.
2. For a single image or scanned PDF, run `scripts/ocr.py`.
3. For text-native PDFs, let `ocr.py` use `pdftotext` first; only use `--force-ocr` when the PDF is a scan or the extracted text is wrong.
4. Inspect output before relying on it. OCR can confuse punctuation, columns, totals, handwriting, low contrast, and small text.
5. If the user needs high confidence, run JSON output and mention low-confidence lines or ambiguous characters.
6. If the user asks for a clean transcript, lightly normalize whitespace but do not silently rewrite wording.

## Tool choices

- Use `scripts/ocr.py` for local images/PDFs and repeatable extraction.
- Use the model `image` tool when the user asks for visual understanding, layout interpretation, or when OCR output alone is not enough.
- Use `pdftotext` directly only for text-native PDFs when no OCR is needed.

## Script behavior

`scripts/ocr.py` supports:

- image inputs: PNG, JPEG, TIFF, BMP, GIF, WebP, HEIC/HEIF
- PDF inputs: embedded text via `pdftotext`, scanned pages via `pdftoppm` + macOS Vision OCR
- directory inputs: processes supported files in sorted order
- output formats: `text`, `json`, `markdown`

Dependencies:

- macOS `swift` + Vision framework for image OCR
- `pdftotext` for text-native PDFs
- `pdftoppm` for scanned PDF rasterization

If a dependency is missing, install it or fall back to the `image` tool for one-off extraction.

## Reporting results

Be explicit about uncertainty:

- Say “OCR reads…” or “I extracted…” rather than presenting uncertain OCR as ground truth.
- Preserve line breaks when they matter, especially for receipts, forms, addresses, code, and tables.
- For sensitive documents, summarize only what the user requested and avoid exposing unnecessary personal data.
- For financial/legal/medical text, flag that OCR may need human verification before decisions.
