# Processing pipeline, concurrency and batching

This document shows what actually happens to a file from upload to `result.json`, and
exactly how much runs at once. All numbers match the current code (`src/extract.rs`,
`src/service.rs`, `src/model.rs`) with default limits.

## 1. Layers: service → one process per job → format router

```
 HTTP client / CLI
        │  multipart upload (streamed) or a local path
        ▼
 ┌─────────────────────────────────────────────────────────────────────────┐
 │ Service (Axum + SQLite)                                                 │
 │  • streams input to data/jobs/<id>/input                                │
 │  • durable queue: queued / running / completed / failed / cancelled    │
 │  • dispatcher: at most limits.workers jobs at once  (default 2)        │
 └───────────────┬─────────────────────────────────────────────────────────┘
                 │ spawns ONE isolated worker process per job
                 ▼
 ┌─────────────────────────────────────────────────────────────────────────┐
 │ Worker process: doc-extractor worker --job-dir <dir>                    │
 │  • process group killed on cancel/timeout (native children included)    │
 │  • rlimits: RLIMIT_CPU = timeout_seconds, RLIMIT_FSIZE = output+64 MiB  │
 │  • memory: RLIMIT_AS on Linux, sampled process-tree RSS on macOS        │
 │  • one job per process → jobs never share memory or parser state        │
 └───────────────┬─────────────────────────────────────────────────────────┘
                 │ extract::extract()
                 ▼
        format router (by filename extension)
        ├── pdf            → pdf()   : 100-page batches, parallel workers   → §2
        ├── csv | tsv      → delimited(): 50k-row / 250k-cell chunk units   → §5
        ├── xlsx/xls/ods…  → spreadsheet(): one unit per sheet (sequential)
        ├── docx/pptx/odt… → office(): one unit per logical section/slide
        └── png/jpg/…      → figure asset + optional OCR/vision (per image)
```

## 2. PDF: how pages are actually batched and run concurrently

Not all pages at once. Pages are grouped into **100-page batches** (`PDF_BATCH = 100`), and
up to **`limits.pdf_workers` batches run in parallel** (default 4, clamped to 1..8). Each
worker thread processes its batches one after another; pages inside a batch are processed
strictly in order by that thread. The coordinator merges everything in page order.

```
 3000-page PDF, default pdf_workers = 4
 pages:   0────99   100───199   200───299   300───399   …   2900──2999
            │          │           │           │                │
 planner:  build batch list, skip pages whose units/uN.json already exists
 (resume)  │          │           │           │                │
           ▼          ▼           ▼           ▼                ▼
 thread A  batch 0    batch 4     batch 8     batch 12   …
 thread B  batch 1    batch 5     batch 9     batch 13   …      static split:
 thread C  batch 2    batch 6     batch 10    batch 14   …      ceil(batches / workers)
 thread D  batch 3    batch 7     batch 11    batch 15   …
           │
           │  per batch, per thread (3 Poppler processes for the whole range):
           │    1. pdftotext -f s -l e -bbox-layout   → XHTML for the range
           │    2. pdfimages -list  -f s -l e          → image → page table
           │    3. pdfimages -png   -f s -l e <dir>    → image files
           │    then for each missing page p (in order):
           │       • parse the page's XML slice → text lines + bounds + table proposals
           │       • attach that page's images from the -list map
           │       • page has no text and OCR enabled → pdftoppm one page → OCR
           │       • write units/u{p+1}.json            (atomic, always)
           │       • write assets/<id> + assets/<id>.json sidecar
           │
           ▼
 coordinator (the original process):
   1. read units/uN.json strictly in page order → document.units
   2. load asset sidecars sorted by seq ((page+1)<<32 + local) → document.assets
   3. write result.json (streamed, size-capped), result.md, result.txt, manifests
```

Consequences of this design:

- **Memory**: bounded per thread by one 100-page XML range plus one page unit at a time;
  never the whole document's pages at once. `result.json` is assembled by streaming the
  `units/uN.json` files, `result.md`/`result.txt` are written unit by unit, and delimited
  inputs over `limits.stream_bytes` release each chunk's blocks after its unit file is
  written (see §4).
- **Resume**: completed pages are skipped before any Poppler call — a fully resumed
  500-page PDF finishes in ~0.5 s.
- **Determinism**: parallel and sequential runs produce byte-identical results (test
  `pdf_parallel_and_sequential_batches_match`). Workers pull batch indices from a shared
  atomic queue (dynamic scheduling), which only changes *when* work runs, never the merged
  order.
- **Failure containment**: one page's render/OCR failure marks that page partial; completed
  pages from other workers are still merged and written before the error is reported. If
  Poppler ever returned fewer `<page>` elements than requested, text is re-fetched per page
  instead of shifting another page's text onto the wrong unit.
- **Checkpoints**: unit files are written by each worker as it goes; resume needs only
  `identity.json` (carrying a `checkpoint_format` version), the unit files and the asset
  sidecars. Workers publish lightweight unit stubs (metadata, warnings, errors — no blocks)
  to a shared progress board, and a coordinator writer thread turns them into complete,
  page-ordered `checkpoint.json`, `assets.json` and `progress.json` snapshots every ~2 s or
  100 completed pages — in both sequential and parallel modes, so a running 3000-page job
  shows live progress instead of 0. A format-version mismatch forces clean re-extraction
  instead of reusing files whose meaning changed.

## 3. OCR concurrency

Each batch thread runs at most one OCR at a time. A process-wide semaphore
(`limits.ocr_threads`, default 4) caps how many Tesseract processes may run *across all
batch threads of that job*, so raising `pdf_workers` cannot oversubscribe the CPU:

```
 thread A ──OCR──┐
 thread B ──OCR──┤   semaphore slots = ocr_threads (4)
 thread C ──OCR──┤   → at most 4 concurrent tesseracts per worker process
 thread D ──OCR──┘
```

Rotation probing can spawn up to three extra Tesseract runs for one image (90°/180°/270°);
those all happen inside the single held slot, so the bound still holds.

**Capacity note**: the limiter is *per worker process*. With `limits.workers = 2` jobs and
`ocr_threads = 4`, up to 8 Tesseract processes can run machine-wide. Size `workers x
ocr_threads` against available cores.

## 4. Non-PDF formats

```
 CSV / TSV (e.g. 700k rows)
   stream records → flush a unit every 50k rows or 250k cells
   unit 1 rows 1-50000 → units/u1.json      (provenance: cell_range "rows 1-50000")
   unit 2 rows 50001-100000 → units/u2.json
   …                                        compact_tables omits redundant cell fields
   → single-threaded; memory bounded per chunk (not per file)
   → inputs > limits.stream_bytes: blocks released after each unit file;
     result/markdown written by streaming units/uN.json one at a time
     (measured live heap: 107 MB peak for a 700k-row CSV vs 915 MB buffered)

 XLSX / ODS      one unit per sheet, sequential, formulas + cached values kept
 DOCX / PPTX     one unit per logical section / slide, sequential, source-order
 images          original bytes retained; OCR/vision per image when enabled
```

## 5. Where the concurrency knobs live

| Layer | Knob | Default | Range / effect |
|---|---|---|---|
| Jobs in flight | `limits.workers` | 2 | one isolated process per job |
| PDF batch threads | `limits.pdf_workers` | 4 | 1..8; one 100-page batch per thread |
| Concurrent OCR | `limits.ocr_threads` | 4 | Tesseract slots; machine-wide across worker processes via `flock` slot files in the service data dir (`<data_dir>/ocr-slots`), in-process otherwise |
| CSV chunk size | constants | 50k rows / 250k cells | per-unit memory bound |
| Streamed tables | `limits.stream_bytes` | 16 MiB | delimited inputs above this release blocks after each unit; assembly streams unit files |
| PDF batch size | constant | 100 pages | one Poppler range per batch |
| Progress snapshots | ~2 s / 100 units | coordinator writer thread | complete, ordered, versioned; runs in both modes |

## 6. Measured wall times (this Mac, release build, synthetic inputs)

| Workload | 1 worker | 4 workers |
|---|---|---|
| 3000-page text PDF | 29.8 s | **20.7 s** |
| 500-page text PDF | 5.0 s | — |
| 500-page resume (no changes) | 0.46 s | — |
| 400k-row CSV (full tables) | 2.97 s | n/a (single-threaded) |
| 400k-row CSV (`compact_tables`) | 1.5 s | n/a |

See `perf-large-inputs.json` for the full record and `DEPLOYMENT.md` for sizing guidance.
