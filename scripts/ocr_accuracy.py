#!/usr/bin/env python3
"""Run the Hindi + English OCR stress fixture and score it against its ground-truth TXT.

    python3 scripts/ocr_accuracy.py
    python3 scripts/ocr_accuracy.py --pdf fixtures/hindi-english-complex.pdf \
        --truth fixtures/hindi-english-complex.ground-truth.txt

Writes docs/hindi-english-ocr.md and docs/hindi-english-ocr.json. The report includes character
and word error rates, critical-field recall, Devanagari/Latin coverage, table structure findings
and every unit error/warning. Exit status is 1 only with --strict when a requested threshold fails.
"""

import argparse
import json
import pathlib
import re
import subprocess
import tempfile
import time
import unicodedata

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_PDF = ROOT / "fixtures/hindi-english-complex.pdf"
DEFAULT_TRUTH = ROOT / "fixtures/hindi-english-complex.ground-truth.txt"
DEFAULT_BINARY = ROOT / "target/debug/doc-extractor"
DEFAULT_MD = ROOT / "docs/hindi-english-ocr.md"
DEFAULT_JSON = ROOT / "docs/hindi-english-ocr.json"

DEVANAGARI_DIGITS = str.maketrans("०१२३४५६७८९", "0123456789")

MARKERS = {
    0: [
        "सत्यमेव जयते",
        "माध्यमिक शिक्षा परिषद",
        "CERTIFICATE-CUM-MARKS SHEET",
        "उदेश शर्मा",
        "उदेश शर्मा (Udesh Sharma)",
        "7534469",
        "17 November 2000",
        "ST MARY'S C GORAKHPUR",
        "PASSED",
        "हिंदी",
        "English",
        "Mathematics",
        "83",
        "A1",
        "सत्यापित",
    ],
    1: [
        "जिला शिक्षा कार्यालय",
        "District Education Office",
        "SCHOOL INSPECTION REPORT",
        "92 प्रतिशत",
        "2,450",
        "Key Findings",
        "Computer Lab",
        "Needs Improvement",
        "डॉ. अनीता वर्मा",
        "APPROVED",
    ],
    2: [
        "COMPLIANCE CERTIFICATE",
        "OCR परीक्षण",
        "Hindi + English",
        "90 degrees",
        "Recovered",
        "Registrar",
        "TESTED",
    ],
}

EXPECTED_TABLE_TOKENS = ["100", "40", "28", "68", "B2", "50", "29", "79", "B1", "83", "A1", "2,450"]


def normalize(text):
    text = unicodedata.normalize("NFC", text)
    text = text.translate(DEVANAGARI_DIGITS)
    for old, new in [("’", "'"), ("‘", "'"), ("“", '"'), ("”", '"'), ("—", "-"), ("–", "-")]:
        text = text.replace(old, new)
    return re.sub(r"\s+", " ", text).strip()


def compact(text):
    return "".join(ch for ch in normalize(text) if ch.isalnum() or "\u0900" <= ch <= "\u097f")


def levenshtein(a, b):
    if len(a) < len(b):
        a, b = b, a
    previous = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        current = [i]
        for j, cb in enumerate(b, 1):
            current.append(min(current[-1] + 1, previous[j] + 1, previous[j - 1] + (ca != cb)))
        previous = current
    return previous[-1]


def rate(a, b):
    return levenshtein(a, b) / max(len(b), 1)


def words(text):
    return normalize(text).split()


def load_truth(path):
    text = path.read_text(encoding="utf-8")
    pages = re.split(r"=== PAGE \d+ ===", text)
    return [page.strip() for page in pages if page.strip()]


def run_extraction(binary, pdf, options):
    work = pathlib.Path(tempfile.mkdtemp(prefix="hindi-english-ocr-"))
    options_path = work / "options.json"
    options_path.write_text(json.dumps(options), encoding="utf-8")
    output = work / "result"
    started = time.monotonic()
    process = subprocess.run(
        [str(binary), "extract", str(pdf), "--output", str(output), "--options", str(options_path)],
        capture_output=True,
        text=True,
    )
    elapsed = time.monotonic() - started
    if not (output / "result.json").exists():
        raise SystemExit(
            f"extraction failed (exit {process.returncode}):\n{process.stdout}\n{process.stderr}\n"
            f"work dir: {work}"
        )
    document = json.loads((output / "result.json").read_text(encoding="utf-8"))
    return document, elapsed, output, process


def text_of(unit):
    # Blocks are grouped into lines; join with spaces so tables/paragraphs compare as text.
    return " ".join(block.get("text", "") for block in unit.get("blocks", []) if block.get("text"))


def evaluate(document, truth, strict):
    page_units = [unit for unit in document.get("units", []) if unit.get("kind") == "page"]
    pages = []
    total_cer_weight = 0
    total_cer_error = 0
    total_alnum_weight = 0
    total_alnum_error = 0
    total_wer_weight = 0
    total_wer_error = 0
    all_ocr = []

    for index, unit in enumerate(page_units):
        ocr_text = text_of(unit)
        all_ocr.append(ocr_text)
        expected = truth[index] if index < len(truth) else ""
        ocr_alnum = compact(ocr_text)
        truth_alnum = compact(expected)
        ocr_words = words(ocr_text)
        truth_words = words(expected)
        cer = rate(ocr_alnum, truth_alnum)
        wer = rate(ocr_words, truth_words)
        total_cer_error += levenshtein(ocr_alnum, truth_alnum)
        total_cer_weight += max(len(truth_alnum), 1)
        total_alnum_error += levenshtein(ocr_alnum, truth_alnum)
        total_alnum_weight += max(len(truth_alnum), 1)
        total_wer_error += levenshtein(ocr_words, truth_words)
        total_wer_weight += max(len(truth_words), 1)
        devanagari = sum(1 for ch in ocr_text if "\u0900" <= ch <= "\u097f")
        latin = sum(1 for ch in ocr_text if ch.isascii() and ch.isalpha())
        marker_results = []
        for marker in MARKERS.get(index, []):
            marker_results.append({"marker": marker, "found": normalize(marker) in normalize(ocr_text)})
        tables = [
            block
            for block in unit.get("blocks", [])
            if block.get("table")
        ]
        pages.append(
            {
                "page": index + 1,
                "name": unit.get("name"),
                "ocr_chars": len(ocr_text),
                "truth_chars": len(compact(expected)),
                "cer": round(cer, 4),
                "wer": round(wer, 4),
                "devanagari_chars": devanagari,
                "latin_chars": latin,
                "blocks": len(unit.get("blocks", [])),
                "table_blocks": len(tables),
                "errors": unit.get("errors", []),
                "warnings": unit.get("warnings", []),
                "markers": marker_results,
                "text": ocr_text,
            }
        )

    overall_cer = total_cer_error / max(total_cer_weight, 1)
    overall_wer = total_wer_error / max(total_wer_weight, 1)
    ocr_all = "\n".join(all_ocr)
    truth_all = "\n".join(truth)
    devanagari_ocr = sum(1 for ch in ocr_all if "\u0900" <= ch <= "\u097f")
    devanagari_truth = sum(1 for ch in truth_all if "\u0900" <= ch <= "\u097f")
    latin_ocr = sum(1 for ch in ocr_all if ch.isascii() and ch.isalpha())
    latin_truth = sum(1 for ch in truth_all if ch.isascii() and ch.isalpha())
    markers_total = sum(len(page["markers"]) for page in pages)
    markers_found = sum(1 for page in pages for marker in page["markers"] if marker["found"])
    table_blocks = sum(page["table_blocks"] for page in pages)
    table_tokens_found = sum(
        1 for token in EXPECTED_TABLE_TOKENS if normalize(token) in normalize(ocr_all)
    )
    findings = []
    if table_blocks == 0:
        findings.append(
            "Scanned tables are not promoted to structured `Table` blocks; cells remain linearized OCR text."
        )
    if table_tokens_found < len(EXPECTED_TABLE_TOKENS):
        findings.append(
            f"Only {table_tokens_found}/{len(EXPECTED_TABLE_TOKENS)} expected table tokens were found; "
            "small grid text is the weakest region."
        )
    if devanagari_ocr == 0:
        findings.append("No Devanagari characters were recovered; check `languages: eng+hin`.")
    elif devanagari_ocr < devanagari_truth * 0.5:
        findings.append(
            f"Only {devanagari_ocr}/{devanagari_truth} Devanagari characters were recovered."
        )
    if any(page["errors"] for page in pages):
        findings.append("At least one page still has extraction errors.")
    if not markers_found == markers_total:
        missing = [
            marker["marker"]
            for page in pages
            for marker in page["markers"]
            if not marker["found"]
        ]
        findings.append("Missing critical fields: " + ", ".join(missing))

    result = {
        "pdf": str(document.get("filename")),
        "status": document.get("status"),
        "pages": len(page_units),
        "expected_pages": len(truth),
        "elapsed_ms": document.get("elapsed_ms"),
        "units": pages,
        "overall": {
            "cer": round(overall_cer, 4),
            "wer": round(overall_wer, 4),
            "devanagari_ocr": devanagari_ocr,
            "devanagari_truth": devanagari_truth,
            "latin_ocr": latin_ocr,
            "latin_truth": latin_truth,
            "markers_found": markers_found,
            "markers_total": markers_total,
            "table_blocks": table_blocks,
            "table_tokens_found": table_tokens_found,
            "table_tokens_total": len(EXPECTED_TABLE_TOKENS),
            "document_errors": len(document.get("errors", [])),
            "unit_errors": sum(len(page["errors"]) for page in pages),
            "warnings": document.get("warnings", []),
        },
        "findings": findings,
    }
    if strict and (overall_cer > 0.25 or table_blocks == 0 or markers_found < markers_total):
        result["strict_failure"] = True
    return result


def markdown(report):
    lines = [
        "# Hindi + English OCR stress test",
        "",
        f"Fixture: `{report['pdf']}` · **{report['pages']} pages** · document status `{report['status']}` · "
        f"extraction {report['elapsed_ms']} ms.",
        "",
        "Ground truth: `fixtures/hindi-english-complex.ground-truth.txt`.",
        "",
        "## Overall accuracy",
        "",
        "| Metric | Result |",
        "|---|---|",
        f"| Character error rate (letters/digits) | **{report['overall']['cer'] * 100:.2f}%** |",
        f"| Word error rate | **{report['overall']['wer'] * 100:.2f}%** |",
        f"| Critical fields found | **{report['overall']['markers_found']}/{report['overall']['markers_total']}** |",
        f"| Devanagari characters (OCR / truth) | {report['overall']['devanagari_ocr']} / {report['overall']['devanagari_truth']} |",
        f"| Latin characters (OCR / truth) | {report['overall']['latin_ocr']} / {report['overall']['latin_truth']} |",
        f"| Structured table blocks | **{report['overall']['table_blocks']}** |",
        f"| Expected table tokens found | {report['overall']['table_tokens_found']}/{report['overall']['table_tokens_total']} |",
        f"| Unit errors | {report['overall']['unit_errors']} |",
        "",
        "## Per page",
        "",
        "| Page | OCR chars | CER | WER | Blocks | Tables | Devanagari | Latin | Critical fields |",
        "|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for page in report["units"]:
        found = sum(1 for marker in page["markers"] if marker["found"])
        lines.append(
            f"| {page['page']} | {page['ocr_chars']} | {page['cer'] * 100:.2f}% | "
            f"{page['wer'] * 100:.2f}% | {page['blocks']} | {page['table_blocks']} | "
            f"{page['devanagari_chars']} | {page['latin_chars']} | {found}/{len(page['markers'])} |"
        )
    lines += ["", "## Critical fields", "", "| Page | Field | Found |", "|---:|---|:---:|"]
    for page in report["units"]:
        for marker in page["markers"]:
            lines.append(
                f"| {page['page']} | `{marker['marker']}` | {'yes' if marker['found'] else '**no**'} |"
            )
    lines += ["", "## Findings", ""]
    if report["findings"]:
        lines.extend(f"- {finding}" for finding in report["findings"])
    else:
        lines.append("- No structural findings; inspect the per-field table above for residual errors.")
    lines += [
        "",
        "## Method",
        "",
        "- OCR uses `languages: eng+hin`, page rendering at 150 DPI, automatic rotation and `page_images: all`.",
        "- Character/word rates ignore punctuation, normalize whitespace, map Devanagari digits to ASCII and compare against the committed TXT.",
        "- The PDF is raster-only by design (a scanner simulation), so every character is produced by OCR rather than a PDF text layer.",
        "- Page 3 is rotated 90° to exercise automatic rotation recovery.",
        "",
    ]
    return "\n".join(lines).rstrip() + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pdf", type=pathlib.Path, default=DEFAULT_PDF)
    parser.add_argument("--truth", type=pathlib.Path, default=DEFAULT_TRUTH)
    parser.add_argument("--binary", type=pathlib.Path, default=DEFAULT_BINARY)
    parser.add_argument("--out-md", type=pathlib.Path, default=DEFAULT_MD)
    parser.add_argument("--out-json", type=pathlib.Path, default=DEFAULT_JSON)
    parser.add_argument("--strict", action="store_true")
    parser.add_argument("--languages", default="eng+hin")
    parser.add_argument("--no-rotate", action="store_true")
    args = parser.parse_args()

    if not args.binary.exists():
        raise SystemExit(f"build the extractor first: {args.binary}")
    if not args.pdf.exists() or not args.truth.exists():
        raise SystemExit("fixture PDF or ground-truth TXT is missing")

    options = {
        "ocr": True,
        "languages": args.languages,
        "ocr_auto_rotate": not args.no_rotate,
        "page_images": "all",
        "chunk_chars": 1000,
        "chunk_overlap": 80,
        "result_format": "text",
    }
    document, elapsed, _, _ = run_extraction(args.binary, args.pdf, options)
    truth = load_truth(args.truth)
    report = evaluate(document, truth, args.strict)
    report["elapsed_wall_s"] = round(elapsed, 2)
    report["options"] = options

    args.out_json.parent.mkdir(parents=True, exist_ok=True)
    args.out_json.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    args.out_md.write_text(markdown(report), encoding="utf-8")

    print(f"status={report['status']} pages={report['pages']}/{report['expected_pages']}")
    print(
        f"CER={report['overall']['cer'] * 100:.2f}% WER={report['overall']['wer'] * 100:.2f}% "
        f"markers={report['overall']['markers_found']}/{report['overall']['markers_total']} "
        f"tables={report['overall']['table_blocks']}"
    )
    for finding in report["findings"]:
        print("FINDING:", finding)
    print(f"wrote {args.out_md} and {args.out_json}")
    if args.strict and report.get("strict_failure"):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
