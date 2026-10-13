# Code review — 6d905f4 vs `docs/PIPELINE.md` (13 October 2026)

Reviewed areas: end-to-end memory bounds, final JSON/Markdown assembly, Poppler page-range
flags, scheduling, mixed-page OCR, checkpoint validity and cancellation of descendant
processes. Line references are against the reviewed commit; every confirmed issue now has a
fix and a test or measurement.

## Confirmed issues and resolutions

| # | Issue (at `6d905f4`) | Resolution |
|---|---|---|
| 1 | Parallel PDF runs wrote no checkpoint or progress: workers ran with `write_checkpoint = false` (`src/extract.rs` parallel branch, `parallel_context`) and the coordinator wrote nothing until `extract()` finished, so a running job reported `progress: 0` for its whole runtime (`src/service.rs` job progress is read from `progress.json`). | Workers publish unit stubs to a shared `ProgressBoard`; a coordinator writer thread refreshes complete, page-ordered `checkpoint.json`/`assets.json`/`progress.json` every ~2 s or 100 units, in both modes. Measured: a 500-page parallel job reports 0 → 101 → 232 → 364 → 487 → 500 while running. |
| 2 | `checkpoint.json` was incomplete and unordered (sequential runs never included reused pages — they lived only in the `slots` vector) and unversioned: resume depended implicitly on the sidecar sequence scheme (`(page + 1) << 32`). | Snapshots now list every completed page in page order; `identity.json` carries `checkpoint_format` (2) and a mismatch forces clean re-extraction; `units/uN.json` remains the resume source. |
| 3 | A batch-worker error discarded every completed unit: `for result in results { result?; }` ran before the slot merge, so one failing batch dropped finished pages from the returned document and `result.json`. | Merge-before-error: completed units (and asset sidecars) are merged first, missing pages are recorded as a warning, then the error is reported. |
| 4 | The render path could abort the whole document: `pdfinfo`, `pdftoppm` and `c.figure` used `?` in the OCR-only branch, unlike the embedded-image path which records per-unit failures. | `render_page_and_ocr` returns errors to the caller, which records a unit failure; the page is marked partial and other pages survive. The render PNG is also removed instead of accumulating in the job dir. |
| 5 | Static batch scheduling (`batches.chunks(per_worker)`) let one image/OCR-heavy batch idle the other workers. | Workers pull batch indices from a shared atomic queue; merge order is unchanged, so output stays deterministic (`pdf_parallel_and_sequential_batches_match`). |
| 6 | Memory/streaming gap: `document.units` held every unit and cell in RAM and `result.md`/`result.txt` were built as whole in-memory strings, so multi-million-row CSVs scaled with the file (~0.9 GB for 700k rows); `PIPELINE.md` implied per-chunk bounds. | Delimited inputs above `limits.stream_bytes` (16 MiB default) release each chunk's blocks after its unit file is written; `result.json` is assembled by streaming unit files and `result.md`/`result.txt` are written unit by unit. Measured live heap for a 700k-row CSV: 107 MB peak / 0 MB final versus 915 MB / 496 MB buffered. |
| 7 | The OCR limiter was a process-local `OnceLock` semaphore; two jobs with `ocr_threads = 4` could run 8 Tesseracts machine-wide. | Worker processes receive `DOC_OCR_SLOTS_DIR` (inside the service data dir) and take exclusive `flock` slots there; the in-process semaphore still applies when no shared dir is configured. E2E check: two concurrent OCR jobs with `ocr_threads = 1` hold at most one Tesseract. |
| 8 | Batch XML page attribution was positional (`page_slices.get(i - batch_start)`); a page-count mismatch would silently shift text onto the wrong page. Confirmed correct for digital and empty pages (e.g. `long.pdf` unit *k* text matches page *k + 1* for k ∈ {0, 1, 49, 99, 100, 101, 129}), but unguarded. | A batch whose `<page>` count does not match its range falls back to per-page `pdftotext` and records a warning instead of misattributing text. |
| 9 | Minor: killed atomic writes left `.tmp-*` files that counted toward the disk quota forever; cancellation after `try_wait()` races PID reuse (no pidfd on macOS). | Temp files are swept at extraction start (and e2e asserts none remain after cancellation). The PID-reuse race remains a documented macOS limitation; atomic renames bound its impact to a rare wrong-pid signal. |

## Verified as correct

- Poppler page-range flags: `pdftotext`, `pdfimages`, `pdfinfo` and `pdftoppm` all use `-f`/`-l`
  consistently for batches and single pages.
- The asset API reads `assets.json` (small manifest) rather than reparsing `result.json`.
- Cancellation kills the worker's process group (`setsid` + `kill(-pid)`), reaching Poppler,
  Tesseract and render children; the e2e suite now asserts no descendants survive.
- Parallel and sequential batch runs produce byte-identical results.

## Follow-up measurements (see `VALIDATION.md`, `perf-large-inputs.json`)

Real-shape synthetic runs on the M1 8 GB host, release build, four batch workers and four OCR
slots: 2000-page digital PDF 13.1 s; 80 scanned pages with OCR 47.4 s (OCR 42 s); 100-page
mixed PDF 24.0 s; 30-page image-heavy PDF with 90 panels 38.4 s; 2M-row/87 MB CSV 20.1 s
(345.6 MB result with `compact_tables` versus 1.02 GB full). Crash recovery with 503/2000
pages complete resumed to completion in 11.5 s after a service+worker `SIGKILL`; cancelling a
running OCR job left no descendants or temp files. Per-stage timings are recorded in each
job's `timings.json` and in the benchmark report.
