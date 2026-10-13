#!/usr/bin/env python3
"""Extraction accuracy report against the committed synthetic fixtures.

Every fixture has known content (see scripts/generate-fixtures.py), so "accuracy" here is
measurable rather than anecdotal:

- table-cell accuracy: exact `raw` match for CSV/TSV/XLSX/XLS/ODS and geometric PDF tables
- page-text accuracy: exact per-page native text for digital/long/pages-images PDFs
- content checks: expected strings/order/structure for Office, PDF, diagram, archive,
  email, markup and negative/robustness cases (macro sentinel absent, no entity leak, ...)
- OCR accuracy: character/word error rate against known raster text (English; gated on
  the installed tesseract languages)

Outputs `docs/accuracy-results.json` and `docs/ACCURACY.md`. Run:
    scripts/cargo-local.sh build --release --locked
    python3 scripts/accuracy.py
"""
import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
FIX = ROOT / "fixtures"
BIN = next((p for p in [ROOT / "target/release/doc-extractor", ROOT / "target/debug/doc-extractor"] if p.exists()), None)


def extract(fixture, flags=(), out_root=None):
    """Run the CLI once for one fixture; returns (document dict or None, error string)."""
    out = pathlib.Path(tempfile.mkdtemp(prefix="accuracy-", dir=out_root))
    result = subprocess.run(
        [str(BIN), "extract", str(FIX / fixture), "--output", str(out), *flags],
        capture_output=True,
        text=True,
        timeout=300,
    )
    result_path = out / "result.json"
    if not result_path.exists():
        return None, (result.stderr or result.stdout or "no result.json").strip()[:200]
    return json.loads(result_path.read_text()), ""


# ---------------------------------------------------------------- helpers
def blocks(doc):
    return [b for u in doc["units"] for b in u["blocks"]]


def texts(doc):
    out = []
    for b in blocks(doc):
        if b.get("table"):
            out.extend(c["raw"] for c in b["table"]["cells"])
        elif b.get("text"):
            out.append(b["text"])
    return out


def normalized(value):
    return " ".join(value.split())


def joined(doc):
    return normalized(" ".join(texts(doc)))


def ocr_text(doc):
    parts = [
        b["text"]
        for b in blocks(doc)
        if b.get("kind") == "text" and b.get("provenance", {}).get("method") == "ocr"
    ]
    if not parts:  # word-level blocks may lack the marker on some providers; fall back
        parts = [t for t in texts(doc) if t not in ("Original PDF preserves vector content",)]
    return normalized(" ".join(parts))


def tables(doc):
    return [b["table"] for b in blocks(doc) if b.get("table")]


def cells_by_key(doc):
    found = {}
    for table in tables(doc):
        for cell in table["cells"]:
            found[(cell["row"], cell["column"])] = cell
    return found


def diagrams(doc):
    return [b["diagram"] for b in blocks(doc) if b.get("diagram")]


def levenshtein(a, b):
    row = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        nxt = [i]
        for j, y in enumerate(b, 1):
            nxt.append(min(nxt[-1] + 1, row[j] + 1, row[j - 1] + (x != y)))
        row = nxt
    return row[-1]


def error_rate(truth, got):
    ref, hyp = normalized(truth), normalized(got)
    return levenshtein(ref, hyp) / max(len(ref), 1)


def word_error_rate(truth, got):
    ref, hyp = normalized(truth).split(), normalized(got).split()
    return levenshtein(ref, hyp) / max(len(ref), 1)


def status_ok(doc, allowed):
    return doc.get("status") in allowed


def has_error_entry(doc):
    return bool(doc.get("errors")) or any(u.get("errors") for u in doc["units"])


# ---------------------------------------------------------------- checks
# kind: cells | pages | contains | order | absent | count | diagram | status | ocr | recall
CASES = [
    # delimited tables -> cell accuracy
    ("CSV cells (quoting, multiline, leading zeros)", "quoted.csv", (), "cells", [
        (0, 0, "name"), (0, 1, "notes"), (0, 2, "number"),
        (1, 0, "Ada"), (1, 1, "first, second"), (1, 2, "0012"),
        (2, 0, "Lin"), (2, 1, "two\nlines"), (2, 2, "3.5"),
    ]),
    ("TSV cells (flexible rows)", "flexible.tsv", (), "cells", [
        (0, 0, "one"), (0, 1, "two"), (1, 0, "three"),
    ]),
    ("XLSX cells", "workbook.xlsx", (), "cells", [
        (0, 0, "Header"), (1, 0, "2"), (1, 1, "3"), (2, 0, "5"),
    ]),
    ("XLS (BIFF8) cells", "workbook.xls", (), "cells", [
        (0, 0, "Header"), (1, 1, "3"), (2, 0, "5"),
    ]),
    ("XLSM cells (macro-enabled)", "workbook.xlsm", (), "cells", [
        (0, 0, "Header"), (1, 0, "2"), (2, 0, "5"),
    ]),
    ("ODS cells", "workbook.ods", (), "cells", [(0, 0, "42")]),
    ("PDF geometric table cells", "table.pdf", (), "cells", [
        (0, 0, "Name"), (0, 1, "Amount"),
        (1, 0, "Ada"), (1, 1, "12"),
        (2, 0, "Lin"), (2, 1, "34"),
    ]),
    # PDF page text -> page accuracy
    ("Digital PDF pages", "digital.pdf", (), "pages", ["Native page ONE", "Native page TWO"]),
    ("130-page PDF pages", "long.pdf", (), "pages", [f"Long page {i}" for i in range(1, 131)]),
    ("PDF pages with images (attribution)", "pages-images.pdf", (), "pages",
     ["Page one", "Page two", "Page three", "Page four", "Page five", "Page six"]),
    # content checks
    ("XLSX formula + cached value + merged cell", "workbook.xlsx", (), "structure", None),
    ("PDF two-column reading order", "multicolumn.pdf", (), "contains",
     ["Left 1", "Left 2", "Left 3", "Left 4", "Right 1", "Right 2", "Right 3", "Right 4"]),
    ("Word document content", "document.docx", (), "contains",
     ["Word heading", "Merged cell", "Section one", "Section two"]),
    ("ODT content", "document.odt", (), "contains", ["ODT heading", "ODT paragraph"]),
    ("EPUB content", "book.epub", (), "contains", ["EPUB chapter", "EPUB content"]),
    ("Markdown content", "document.md", (), "contains",
     ["Heading", "Native text works offline.", "First", "Second"]),
    ("JSON content", "document.json", (), "contains", ["items", "text", "नमस्ते"]),
    ("XML content", "document.xml", (), "contains", ["Native XML"]),
    ("HTML content (no external fetch)", "document.html", (), "contains",
     ["Heading", "Offline HTML", "link", "Header", "A", "B"]),
    ("RTF content", "document.rtf", (), "contains", ["Native RTF", "Second paragraph"]),
    ("Email content", "message.eml", (), "contains", ["Email text"]),
    ("DBML source retained", "schema.dbml", (), "contains",
     ["Table users", "id int [pk]", "Ref: orders.user_id > users.id"]),
    ("Mermaid source retained", "flow.mmd", (), "contains", ["flowchart TD", "A --> B"]),
    ("Graphviz source retained", "flow.dot", (), "contains", ["digraph G", "A -> B"]),
    ("PlantUML source retained", "flow.puml", (), "contains", ["@startuml", "A -> B"]),
    ("PPTX slide source order", "presentation.pptx", (), "order",
     ["First in source order", "Second in source order"]),
    ("draw.io graph nodes + edge", "diagram.drawio", (), "diagram",
     {"nodes": ["Users", "Orders"], "edges": [("orders", "users")]}),
    ("Compressed draw.io graph", "compressed.drawio", (), "diagram",
     {"nodes": ["Users", "Orders"], "edges": [("orders", "users")]}),
    ("PDF-retained draw.io source", "embedded-diagram.pdf", (), "diagram",
     {"nodes": ["Users", "Orders"], "edges": [("orders", "users")]}),
    ("SVG text", "diagram.svg", (), "contains", ["Users"]),
    ("Hidden OCR layer not duplicated", "hidden-ocr.pdf", ("--ocr",), "count",
     {"text": "Invoice TOTAL 12345", "count": 1}),
    ("XLSM macro code never surfaced", "workbook.xlsm", (), "absent",
     ["MACRO_SENTINEL", "AutoOpen", "vbaProject"]),
    ("XXE entity never resolved", "unsafe.xml", (), "absent", ["root:x", "/etc/passwd"]),
    ("Corrupt PDF reports clean failure", "corrupt.pdf", (), "status", ("unsupported", "failed")),
    ("Unsafe archive rejected or reported", "unsafe.docx", (), "status",
     ("failed", "unsupported", "partial")),
    # OCR accuracy -> CER/WER
    ("OCR clean raster", "scan.png", ("--ocr",), "ocr", "Invoice TOTAL 12345"),
    ("OCR noisy raster", "noisy.png", ("--ocr",), "ocr", "Invoice TOTAL 12345"),
    ("OCR rotated raster (90° probe)", "rotated.png", ("--ocr",), "ocr", "Invoice TOTAL 12345"),
    ("OCR image-only PDF page", "scanned.pdf", ("--ocr",), "ocr", "Invoice TOTAL 12345"),
    ("Mixed page: native + OCR", "mixed.pdf", ("--ocr",), "ocr", "Invoice TOTAL 12345"),
    ("OCR raster ER figure entity text (measured, informational)", "raster-er.png", ("--ocr",), "recall", ["users", "orders"]),
    ("OCR Hindi raster (skipped without hin traineddata)", "hindi.png", ("--ocr", "--languages", "hin"), "ocr", "नमस्ते भारत"),
]


def run_case(name, fixture, flags, kind, spec, out_root, langs):
    if kind == "ocr" and "hin" not in langs and fixture == "hindi.png":
        return {"name": name, "fixture": fixture, "kind": kind, "skipped": "hin traineddata not installed"}
    doc, err = extract(fixture, flags, out_root=out_root)
    started = time.time()
    if doc is None:
        return {"name": name, "fixture": fixture, "kind": kind, "passed": False, "detail": f"extraction failed: {err}"}
    record = {"name": name, "fixture": fixture, "kind": kind}
    if kind == "cells":
        found = cells_by_key(doc)
        wrong = [(r, c, want, found.get((r, c), {}).get("raw")) for r, c, want in spec
                 if found.get((r, c), {}).get("raw") != want]
        record.update(cells_total=len(spec), cells_correct=len(spec) - len(wrong),
                      passed=not wrong, detail="" if not wrong else f"mismatches: {wrong[:6]}")
    elif kind == "pages":
        page_units = [u for u in doc["units"] if u["kind"] == "page"]
        hits = 0
        misses = []
        for index, expected in enumerate(spec):
            unit = next((u for u in page_units if u["index"] == index), None)
            page_texts = [normalized(b["text"]) for b in unit["blocks"] if b.get("text")] if unit else []
            if expected in page_texts:
                hits += 1
            else:
                misses.append({"page": index + 1, "expected": expected, "got": page_texts})
        record.update(pages_total=len(spec), pages_correct=hits, passed=hits == len(spec),
                      detail="" if not misses else f"misses: {misses[:4]}")
    elif kind == "contains":
        hay = joined(doc)
        missing = [s for s in spec if normalized(s) not in hay]
        record.update(passed=not missing, detail="" if not missing else f"missing: {missing[:6]}")
    elif kind == "order":
        hay = joined(doc)
        positions = [hay.find(normalized(s)) for s in spec]
        ok = all(p >= 0 for p in positions) and positions == sorted(positions)
        record.update(passed=ok, detail="" if ok else f"positions: {positions}")
    elif kind == "absent":
        hay = joined(doc)
        leaked = [s for s in spec if s.lower() in hay.lower()]
        record.update(passed=not leaked, detail="" if not leaked else f"leaked: {leaked}")
    elif kind == "count":
        block_texts = [normalized(t) for t in texts(doc)]
        count = sum(1 for t in block_texts if t == normalized(spec["text"]))
        record.update(passed=count == spec["count"], detail=f"occurrences: {count}")
    elif kind == "diagram":
        diagrams_found = diagrams(doc)
        labels = {n["label"] for d in diagrams_found for n in d["nodes"]}
        edges = {(e.get("source"), e.get("target")) for d in diagrams_found for e in d["edges"]}
        missing_nodes = [n for n in spec["nodes"] if n not in labels]
        missing_edges = [e for e in spec["edges"] if e not in edges]
        record.update(passed=not missing_nodes and not missing_edges,
                      detail="" if not missing_nodes and not missing_edges
                      else f"nodes missing {missing_nodes}, edges missing {missing_edges}")
    elif kind == "status":
        record.update(passed=status_ok(doc, spec), detail=f"status={doc.get('status')}")
    elif kind == "structure":
        found = cells_by_key(doc)
        merged = found.get((0, 0), {})
        formula_cell = found.get((2, 0), {})
        ok = (
            merged.get("column_span") == 2
            and formula_cell.get("formula") == "SUM(A2:B2)"
            and formula_cell.get("cached_value") == 5.0
        )
        record.update(passed=ok, detail=f"merge={merged.get('column_span')} formula={formula_cell.get('formula')} cached={formula_cell.get('cached_value')}")
    elif kind == "ocr":
        got = ocr_text(doc)
        cer = error_rate(spec, got)
        wer = word_error_rate(spec, got)
        record.update(passed=cer == 0.0, cer=cer, wer=wer, detail=f"text={got[:80]!r}")
    elif kind == "recall":
        # Measured, not pass/fail: this is OCR text recovery for an inferred-content figure.
        hay = joined(doc).lower()
        hits = [t for t in spec if t in hay]
        record.update(passed=None, recall=len(hits) / len(spec),
                      detail=f"found {hits} of {spec}")
    record["elapsed_ms"] = round((time.time() - started) * 1000)
    return record


def main():
    if BIN is None:
        print("no built binary; run scripts/cargo-local.sh build --release --locked", file=sys.stderr)
        return 1
    langs = []
    try:
        langs = subprocess.run(["tesseract", "--list-langs"], capture_output=True, text=True, timeout=30).stdout.split()[1:]
    except Exception:
        pass
    out_root = pathlib.Path(tempfile.mkdtemp(prefix="accuracy-run-"))
    results = []
    started = time.time()
    for name, fixture, flags, kind, spec in CASES:
        record = run_case(name, fixture, flags, kind, spec, out_root, langs)
        results.append(record)
        mark = "skip" if record.get("skipped") else ("meas" if record.get("passed") is None else ("ok  " if record.get("passed") else "FAIL"))
        detail = record.get("detail", "")
        print(f"[{mark}] {name}{' — ' + record['skipped'] if record.get('skipped') else ''}{' — ' + detail if detail and not record.get('passed') else ''}")
    shutil.rmtree(out_root, ignore_errors=True)

    checks = [r for r in results if r["kind"] not in ("cells", "pages", "ocr", "recall") and "skipped" not in r]
    passed_checks = [r for r in checks if r.get("passed")]
    cell_records = [r for r in results if r["kind"] == "cells"]
    cells_total = sum(r["cells_total"] for r in cell_records)
    cells_correct = sum(r["cells_correct"] for r in cell_records)
    page_records = [r for r in results if r["kind"] == "pages"]
    pages_total = sum(r["pages_total"] for r in page_records)
    pages_correct = sum(r["pages_correct"] for r in page_records)
    ocr_records = [r for r in results if r["kind"] == "ocr" and "skipped" not in r]
    mean_cer = sum(r["cer"] for r in ocr_records) / max(len(ocr_records), 1)
    failures = [r["name"] for r in results if "skipped" not in r and r.get("passed") is False]
    skipped = [r["name"] for r in results if r.get("skipped")]

    evidence = {
        "date": time.strftime("%Y-%m-%d %H:%M"),
        "host": subprocess.run(["sysctl", "-n", "hw.model"], capture_output=True, text=True).stdout.strip() or sys.platform,
        "binary_sha256": hashlib.sha256(BIN.read_bytes()).hexdigest(),
        "tesseract_languages": langs,
        "summary": {
            "content_checks_passed": len(passed_checks),
            "content_checks_total": len(checks),
            "cell_accuracy": {"correct": cells_correct, "total": cells_total,
                              "pct": round(100.0 * cells_correct / max(cells_total, 1), 2)},
            "page_text_accuracy": {"correct": pages_correct, "total": pages_total,
                                   "pct": round(100.0 * pages_correct / max(pages_total, 1), 2)},
            "ocr_mean_cer": round(mean_cer, 4),
            "ocr_cases": len(ocr_records),
            "failures": failures,
            "skipped": skipped,
            "elapsed_s": round(time.time() - started, 1),
        },
        "checks": results,
        "notes": [
            "Synthetic fixtures with known ground truth; real customer scans/handwriting remain unrun.",
            "OCR is measured in English only on this host; the Hindi fixture is skipped unless `hin` traineddata is installed.",
            "Geometric PDF tables and raster/vector diagram graphs are proposals (inferred), not claimed fidelity - "
            "the ER raster entity recall line records how much text OCR recovers, not graph correctness.",
            "Table-cell accuracy compares exact cell `raw` strings (leading zeros and multiline content preserved).",
        ],
    }
    (ROOT / "docs/accuracy-results.json").write_text(json.dumps(evidence, indent=1) + "\n")

    lines = [
        f"# Extraction accuracy — {evidence['date']}",
        "",
        f"Host `{evidence['host']}`, binary `{evidence['binary_sha256'][:16]}…`, "
        f"Tesseract languages: {', '.join(langs) or 'none detected'}.",
        "Synthetic fixtures with known content (see `scripts/generate-fixtures.py`); script: `scripts/accuracy.py`.",
        "",
        "| Metric | Result |",
        "|---|---|",
        f"| Content/structure/integrity checks | **{len(passed_checks)}/{len(checks)}** |",
        f"| Table cells exact (CSV/TSV/XLSX/XLS/XLSM/ODS/PDF) | **{cells_correct}/{cells_total}** ({evidence['summary']['cell_accuracy']['pct']}%) |",
        f"| PDF page texts exact | **{pages_correct}/{pages_total}** ({evidence['summary']['page_text_accuracy']['pct']}%) |",
        f"| OCR mean character error rate | **{mean_cer:.4f}** over {len(ocr_records)} raster cases |",
        "",
        "## OCR detail",
        "",
        "| Case | CER | WER | Extracted |",
        "|---|---|---|---|",
    ]
    for r in ocr_records:
        lines.append(f"| {r['name']} | {r['cer']:.4f} | {r['wer']:.4f} | `{r.get('detail', '')[7:60]}` |")
    if failures:
        lines += ["", "## Failures", ""] + [f"- {name}" for name in failures]
    measured = [r for r in results if r["kind"] == "recall"]
    if measured:
        lines += ["", "## Measured (informational, inferred content)", ""]
        for r in measured:
            lines.append(f"- {r['name']}: recall {r['recall']:.2f} — {r.get('detail', '')}")
    if skipped:
        lines += ["", "## Skipped", ""] + [f"- {name}" for name in skipped]
    lines += ["", "## Limits", ""] + [f"- {n}" for n in evidence["notes"]]
    (ROOT / "docs/ACCURACY.md").write_text("\n".join(lines) + "\n")

    print("\n== summary ==")
    print(json.dumps(evidence["summary"], indent=1))
    print("wrote docs/accuracy-results.json and docs/ACCURACY.md")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
