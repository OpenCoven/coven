---
name: "epub"
description: "Convert EPUB ↔ PDF bidirectionally. EPUB→PDF preserves layout for analysis; PDF→EPUB compresses and reflows for distribution and ereaders."
---

# EPUB Skill

## When to use

### EPUB → PDF
- Convert EPUB books, papers, or distributed documents to PDF for reading and analysis
- Preserve layout and formatting before text extraction via pdftoppm
- Archive EPUB content in a fixed-layout format
- Prepare EPUBs for downstream processing (rendering, analysis, synthesis)

### PDF → EPUB
- Convert papers and PDFs to reflowable EPUB format for ereader distribution
- Compress PDFs into smaller, device-friendly EPUB archives
- Auto-detect chapters and structure for better reading experience
- Redistribute research materials in open formats

## Workflow

### EPUB → PDF (for analysis)

1. **Check for calibre** → install if missing (`brew install calibre`)
2. **Convert EPUB → PDF** using `ebook-convert` with formatting options
3. **Validate output** → confirm PDF rendered correctly
4. **Render pages** → use pdftoppm to extract visual content
5. **Analyze** → pass to PDF skill for text extraction and synthesis

### PDF → EPUB (for distribution)

1. **Check for calibre** → install if missing
2. **Convert PDF → EPUB** using `ebook-convert` with reflowable settings
3. **Auto-detect chapters** → ebook-convert scans for structure
4. **Validate compression** → confirm EPUB is smaller than PDF
5. **Archive** → save EPUB to `research/sources/` for distribution or ereader use

## Usage

### EPUB → PDF (basic)

```bash
ebook-convert input.epub output.pdf
```

### EPUB → PDF (with layout preservation options)

```bash
ebook-convert input.epub output.pdf \
  --pdf-standard-font "sans" \
  --paper-size "a4" \
  --margin-left 20 \
  --margin-right 20 \
  --margin-top 20 \
  --margin-bottom 20 \
  --pdf-mono-family "mono"
```

**Note:** Valid font families are: `serif`, `sans`, `mono` (not Helvetica, etc.)

### PDF → EPUB (basic)

```bash
ebook-convert input.pdf output.epub
```

### PDF → EPUB (with compression)

```bash
ebook-convert input.pdf output.epub \
  --output-profile "tablet" \
  --paper-size "a4"
```

**Note:** `ebook-convert` auto-detects chapters from PDFs. Inspect the output EPUB to verify structure.

### In context (Sage workflow)

#### EPUB → PDF
```bash
# Convert EPUB to PDF for analysis
ebook-convert "/path/to/input.epub" "/path/to/research/sources/output.pdf"

# Verify and check size
ls -lh "/path/to/research/sources/output.pdf"

# Render pages for visual inspection
pdftoppm -png "/path/to/research/sources/output.pdf" "/tmp/preview/page"
```

#### PDF → EPUB
```bash
# Convert PDF to EPUB for distribution
ebook-convert "/path/to/input.pdf" "/path/to/research/sources/output.epub"

# Verify compression
ls -lh "/path/to/input.pdf" "/path/to/research/sources/output.epub"

# (EPUB will typically be 30-50% of PDF size)
```

## Dependencies

**System package:**
```bash
# macOS (Homebrew)
brew install calibre

# Ubuntu/Debian
sudo apt-get install -y calibre

# Fedora/RHEL
sudo dnf install calibre
```

`calibre` includes `ebook-convert` and supports many formats:
- Input: EPUB, PDF, MOBI, AZW, AZW3, HTML, TXT, DOCX, ODT
- Output: EPUB, PDF, MOBI, AZW, AZW3, DOCX, HTML, TXT

## Output conventions

- **Location:** Save files to `research/sources/` with descriptive names
- **Naming:** Use title or slug: `Author-Title.pdf`, `title-yyyy-mm-dd.epub`
- **EPUB→PDF:** Check size (>100 KB typical for books)
- **PDF→EPUB:** Typically 30-50% of PDF size due to reflowable format
- **Cleanup:** Remove temporary files after verification

## Quality checks

### After EPUB → PDF:
1. File exists and readable
2. File size >100 KB (suspiciously small = text-only, may have lost images)
3. `file output.pdf` confirms it's a valid PDF
4. pdftoppm renders first page successfully

### After PDF → EPUB:
1. File exists and readable
2. File size is 30-50% of original PDF (good compression ratio)
3. Unzip the EPUB and inspect `content.opf` for auto-detected chapters
4. Open in ereader or calibre viewer to verify reflowable layout

## Troubleshooting

### EPUB → PDF crashes or produces empty output
- Check if EPUB is corrupt: `unzip -t input.epub`
- Try without formatting options: `ebook-convert input.epub output.pdf`
- If EPUB has missing stylesheets (common), calibre will warn but continue
- Last resort: extract text from EPUB (`unzip -p input.epub` | grep -r "html") and convert to markdown

### PDF → EPUB produces wrong chapter detection
- Run with `--verbose` to see what calibre detected: `ebook-convert input.pdf output.epub --verbose`
- For scanned PDFs (images), conversion may fail or produce text-only EPUB
- Use pdftotext first to extract text, then create EPUB from markdown

### File size issues
- EPUB→PDF: if output is <50 KB, likely text-only (no images captured)
- PDF→EPUB: if output is >150% of input, likely includes images; use `--output-profile "tablet"` for better compression

## Notes

- **Layout preservation (EPUB→PDF):** `ebook-convert` maintains structure, typography, images
- **Reflowable format (PDF→EPUB):** EPUB reflows to device width; fixed-layout PDFs may lose structure
- **Chapter detection (PDF→EPUB):** Automatic; works best on PDFs with clear page breaks or bookmarks
- **Scanned PDFs:** May fail or produce text-only output (no OCR)
- **Large files:** Some books convert to large files (100+ MB PDFs/EPUBs). Normal.

## When conversion fails

- **EPUB is corrupt:** `unzip -t input.epub` (EPUBs are ZIP files)
- **Verbose output:** `ebook-convert input.epub output.pdf --verbose` for error details
- **Unsupported format:** Request original source in different format
- **Manual fallback:** Extract text from EPUB or PDF, save as markdown, document the failure

## Related

- [PDF Skill](/concepts/pdf) — for reading and rendering converted PDFs
- [pdftotext](https://manpages.ubuntu.com/manpages/xenial/man1/pdftotext.1.html) — text extraction from PDF
- Calibre Handbook: https://manual.calibre-ebook.com/

## Tested Conversions

✅ **SHELLFISH.epub → SHELLFISH.pdf** (73 KB → 639 KB, layout preserved, 89 pages)  
✅ **SHELLFISH.pdf → SHELLFISH-test.epub** (639 KB → 262 KB, chapters auto-detected, 24 parts)

---

_EPUB ↔ PDF bidirectional conversion pipeline ready._
_Last tested: 2026-04-29 | Calibre v5.x_
