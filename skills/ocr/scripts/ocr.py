#!/usr/bin/env python3
"""OCR helper for the OpenClaw ocr skill.

- Images: macOS Vision OCR through scripts/macos_vision_ocr.swift.
- PDFs: pdftotext first for digital PDFs; scanned PDFs fall back to pdftoppm + Vision OCR.
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp", ".gif", ".webp", ".heic", ".heif"}


def die(message: str, code: int = 1) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(code)


def run(cmd: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=check)


def require(cmd: str, why: str) -> str:
    found = shutil.which(cmd)
    if not found:
        die(f"missing `{cmd}` ({why})")
    return found


def ocr_image(path: Path, *, languages: str, fast: bool) -> dict[str, Any]:
    swift = shutil.which("swift")
    if not swift:
        die("missing `swift`; image OCR uses macOS Vision via Swift")
    helper = Path(__file__).with_name("macos_vision_ocr.swift")
    cmd = [swift, str(helper), str(path), "--languages", languages]
    if fast:
        cmd.append("--fast")
    proc = run(cmd)
    try:
        result = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        die(f"Vision OCR returned invalid JSON: {exc}\n{proc.stdout}\n{proc.stderr}")
    return result


def pdf_text(path: Path) -> str:
    if not shutil.which("pdftotext"):
        return ""
    proc = run(["pdftotext", "-layout", "-enc", "UTF-8", str(path), "-"], check=False)
    return proc.stdout if proc.returncode == 0 else ""


def pdf_to_images(path: Path, outdir: Path, *, max_pages: int | None) -> list[Path]:
    require("pdftoppm", "needed to rasterize scanned PDFs for OCR")
    prefix = outdir / "page"
    cmd = ["pdftoppm", "-png", "-r", "220"]
    if max_pages:
        cmd.extend(["-f", "1", "-l", str(max_pages)])
    cmd.extend([str(path), str(prefix)])
    proc = run(cmd, check=False)
    if proc.returncode != 0:
        die(f"pdftoppm failed:\n{proc.stderr.strip()}")
    return sorted(outdir.glob("page-*.png"))


def collect_inputs(input_path: Path) -> list[Path]:
    if input_path.is_dir():
        return sorted(p for p in input_path.iterdir() if p.suffix.lower() in IMAGE_EXTS or p.suffix.lower() == ".pdf")
    if input_path.exists():
        return [input_path]
    die(f"input not found: {input_path}")


def process_file(path: Path, *, languages: str, fast: bool, force_ocr: bool, max_pages: int | None, min_text_chars: int) -> list[dict[str, Any]]:
    suffix = path.suffix.lower()
    if suffix in IMAGE_EXTS:
        result = ocr_image(path, languages=languages, fast=fast)
        result.update({"source": str(path), "page": None, "method": "macos-vision"})
        return [result]

    if suffix == ".pdf":
        text = pdf_text(path) if not force_ocr else ""
        if len(text.strip()) >= min_text_chars:
            return [{"source": str(path), "page": None, "method": "pdftotext", "text": text.strip(), "lines": []}]
        with tempfile.TemporaryDirectory(prefix="openclaw-ocr-") as tmp:
            images = pdf_to_images(path, Path(tmp), max_pages=max_pages)
            if not images:
                die(f"no pages rasterized from PDF: {path}")
            pages = []
            for idx, image in enumerate(images, start=1):
                result = ocr_image(image, languages=languages, fast=fast)
                result.update({"source": str(path), "page": idx, "method": "pdftoppm+macos-vision"})
                pages.append(result)
            return pages

    die(f"unsupported file type: {path.suffix or '(none)'}")


def render_text(pages: list[dict[str, Any]]) -> str:
    chunks = []
    multi = len(pages) > 1
    for i, page in enumerate(pages, start=1):
        text = (page.get("text") or "").strip()
        if multi:
            label = f"--- {Path(page['source']).name}"
            if page.get("page"):
                label += f" page {page['page']}"
            label += " ---"
            chunks.append(label)
        chunks.append(text)
    return "\n\n".join(chunk for chunk in chunks if chunk)


def render_markdown(pages: list[dict[str, Any]]) -> str:
    chunks = ["# OCR Output"]
    for page in pages:
        title = Path(page["source"]).name
        if page.get("page"):
            title += f" — page {page['page']}"
        chunks.append(f"\n## {title}\n")
        chunks.append((page.get("text") or "").strip() or "_(no text detected)_")
    return "\n".join(chunks).strip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description="Extract text from images and PDFs.")
    parser.add_argument("input", help="image, PDF, or directory of images/PDFs")
    parser.add_argument("--format", choices=["text", "json", "markdown"], default="text")
    parser.add_argument("--output", "-o", help="write output to a file instead of stdout")
    parser.add_argument("--languages", default="en-US", help="Vision language tags, comma-separated; default: en-US")
    parser.add_argument("--fast", action="store_true", help="use faster/lower-accuracy recognition")
    parser.add_argument("--force-ocr", action="store_true", help="OCR PDFs even when pdftotext finds embedded text")
    parser.add_argument("--max-pages", type=int, help="maximum PDF pages to rasterize/OCR")
    parser.add_argument("--min-text-chars", type=int, default=40, help="minimum pdftotext chars before skipping OCR for PDFs")
    args = parser.parse_args()

    inputs = collect_inputs(Path(args.input).expanduser())
    pages: list[dict[str, Any]] = []
    for path in inputs:
        pages.extend(process_file(path, languages=args.languages, fast=args.fast, force_ocr=args.force_ocr, max_pages=args.max_pages, min_text_chars=args.min_text_chars))

    if args.format == "json":
        output = json.dumps({"pages": pages}, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    elif args.format == "markdown":
        output = render_markdown(pages)
    else:
        output = render_text(pages) + "\n"

    if args.output:
        Path(args.output).write_text(output, encoding="utf-8")
    else:
        print(output, end="")


if __name__ == "__main__":
    main()
