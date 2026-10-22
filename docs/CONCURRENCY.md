# Concurrent PDF extraction — performance and accuracy

Date: 2026-10-21. Synthetic corpus; engine `f17c0def976e74b5…`.

Hardware: `MacBookAir10,1, 8 GB`, platform `darwin`, 8 CPUs.

## Corpus

**20 PDFs / 316 pages**, built from the committed fixtures plus 10 `pdfunite` compositions. OCR is enabled only for documents containing a scanned, mixed or hidden-OCR page.

| # | Document | Pages | OCR | Source components |
|---:|---|---:|:---:|---|
| 1 | `01-digital.pdf` | 2 | no | digital |
| 2 | `02-long.pdf` | 130 | no | long |
| 3 | `03-pages-images.pdf` | 6 | no | pages-images |
| 4 | `04-multicolumn.pdf` | 1 | no | multicolumn |
| 5 | `05-table.pdf` | 1 | no | table |
| 6 | `06-embedded-diagram.pdf` | 1 | no | embedded-diagram |
| 7 | `07-mixed.pdf` | 1 | yes | mixed |
| 8 | `08-scanned.pdf` | 1 | yes | scanned |
| 9 | `09-hidden-ocr.pdf` | 1 | yes | hidden-ocr |
| 10 | `10-corrupt.pdf` | — | no | corrupt |
| 11 | `11-digital-table.pdf` | 3 | no | digital, table |
| 12 | `12-mixed-scanned.pdf` | 2 | yes | mixed, scanned |
| 13 | `13-pages-digital.pdf` | 8 | no | pages-images, digital |
| 14 | `14-multicolumn-table-digital.pdf` | 4 | no | multicolumn, table, digital |
| 15 | `15-hidden-mixed.pdf` | 2 | yes | hidden-ocr, mixed |
| 16 | `16-long-digital.pdf` | 132 | no | long, digital |
| 17 | `17-digital-pages-table.pdf` | 9 | no | digital, pages-images, table |
| 18 | `18-scanned-hidden.pdf` | 2 | yes | scanned, hidden-ocr |
| 19 | `19-diagram-multicolumn.pdf` | 2 | no | embedded-diagram, multicolumn |
| 20 | `20-mixed-table-pages.pdf` | 8 | yes | mixed, table, pages-images |

## Performance

Each configuration was run 3 times against a fresh data directory, interleaved to reduce host drift. The table shows the median run; makespan range is in parentheses.

| Configuration | Workers | PDF workers | Makespan median (range) | Docs/s | Pages/s | Max active | Latency p50 / p95 / max | Peak tree RSS |
|---|---:|---:|---:|---:|---:|---:|---|---:|
| default | 2 | 4 | 22.5 s (20.9–23.4) | 0.89 | 14.0 | 2 | 7.3 / 13.4 / 22.2 s | 104 MB |
| tuned | 6 | 2 | 17.1 s (16.9–17.1) | 1.17 | 18.5 | 6 | 4.9 / 9.3 / 16.7 s | 158 MB |

Peak RSS is sampled from the server process tree (`ps`, every ~0.25 s); short-lived Poppler/Tesseract peaks may be missed, and macOS RSS includes allocator-resident pages. Latency/RSS are from the median run; makespan and throughput use the median.

## Accuracy

| Metric | Result |
|---|---|
| Documents passing all checks | **20/20** |
| Page counts correct | **20/20** |
| Exact page text | **311/311** (100.0%) |
| Content markers | **71/71** (100.0%) |
| PDF table cells exact | **30/30** (100.0%) |
| OCR mean / max CER | **0.0000 / 0.0000** over 6 cases |
| Invoice occurrences exact (OCR + hidden text) | **7/7** |
| Retained diagram source | **1/1** |
| Corrupt PDF reported cleanly | **1/1** |

Per-document results (median tuned run; accuracy was identical in all 6 runs: true):

| Document | Status | Pages | Page text | Markers | Table | OCR CER | Pass |
|---|---|---:|---:|---:|---:|---:|:---:|
| `01-digital.pdf` | partial | 2 | 2/2 | 2/2 | — | — | yes |
| `02-long.pdf` | partial | 130 | 130/130 | 4/4 | — | — | yes |
| `03-pages-images.pdf` | partial | 6 | 6/6 | 2/2 | — | — | yes |
| `04-multicolumn.pdf` | partial | 1 | 1/1 | 4/4 | — | — | yes |
| `05-table.pdf` | partial | 1 | — | 4/4 | ok | — | yes |
| `06-embedded-diagram.pdf` | partial | 1 | 1/1 | 0/0 | — | — | yes |
| `07-mixed.pdf` | partial | 1 | 1/1 | 2/2 | — | 0.0000 | yes |
| `08-scanned.pdf` | partial | 1 | 1/1 | 1/1 | — | 0.0000 | yes |
| `09-hidden-ocr.pdf` | partial | 1 | 1/1 | 1/1 | — | — | yes |
| `10-corrupt.pdf` | failed | 0 | — | 0/0 | — | — | yes |
| `11-digital-table.pdf` | partial | 3 | 2/2 | 6/6 | ok | — | yes |
| `12-mixed-scanned.pdf` | partial | 2 | 2/2 | 2/2 | — | 0.0000 | yes |
| `13-pages-digital.pdf` | partial | 8 | 8/8 | 4/4 | — | — | yes |
| `14-multicolumn-table-digital.pdf` | partial | 4 | 3/3 | 10/10 | ok | — | yes |
| `15-hidden-mixed.pdf` | partial | 2 | 2/2 | 2/2 | — | 0.0000 | yes |
| `16-long-digital.pdf` | partial | 132 | 132/132 | 6/6 | — | — | yes |
| `17-digital-pages-table.pdf` | partial | 9 | 8/8 | 8/8 | ok | — | yes |
| `18-scanned-hidden.pdf` | partial | 2 | 2/2 | 1/1 | — | 0.0000 | yes |
| `19-diagram-multicolumn.pdf` | partial | 2 | 2/2 | 4/4 | — | — | yes |
| `20-mixed-table-pages.pdf` | partial | 8 | 7/7 | 8/8 | ok | 0.0000 | yes |

## Per-document timing (tuned run)

| Document | Queue | Run | Total | Pages | Result bytes |
|---|---:|---:|---:|---:|---:|
| `01-digital.pdf` | 0.3 s | 3.0 s | 3.3 s | 2 | 3,197 |
| `02-long.pdf` | 0.3 s | 4.6 s | 4.9 s | 130 | 72,064 |
| `03-pages-images.pdf` | 0.2 s | 3.0 s | 3.2 s | 6 | 6,663 |
| `04-multicolumn.pdf` | 0.2 s | 1.4 s | 1.6 s | 1 | 6,546 |
| `05-table.pdf` | 0.2 s | 1.4 s | 1.6 s | 1 | 5,580 |
| `06-embedded-diagram.pdf` | 0.2 s | 1.4 s | 1.6 s | 1 | 3,985 |
| `07-mixed.pdf` | 1.6 s | 3.2 s | 4.8 s | 1 | 4,565 |
| `08-scanned.pdf` | 1.6 s | 4.3 s | 5.8 s | 1 | 4,235 |
| `09-hidden-ocr.pdf` | 1.5 s | 3.2 s | 4.7 s | 1 | 3,203 |
| `10-corrupt.pdf` | 1.5 s | 1.6 s | 3.2 s | 0 | 1,423 |
| `11-digital-table.pdf` | — | — | 3.1 s | 3 | 6,654 |
| `12-mixed-scanned.pdf` | 3.1 s | 5.3 s | 8.4 s | 2 | 6,545 |
| `13-pages-digital.pdf` | 4.7 s | 2.0 s | 6.7 s | 8 | 7,731 |
| `14-multicolumn-table-digital.pdf` | 4.7 s | 1.1 s | 5.8 s | 4 | 11,077 |
| `15-hidden-mixed.pdf` | 5.8 s | 3.5 s | 9.3 s | 2 | 5,509 |
| `16-long-digital.pdf` | 6.7 s | 2.6 s | 9.3 s | 132 | 73,162 |
| `17-digital-pages-table.pdf` | 5.8 s | 2.0 s | 7.8 s | 9 | 11,188 |
| `18-scanned-hidden.pdf` | 3.1 s | 5.3 s | 8.4 s | 2 | 5,179 |
| `19-diagram-multicolumn.pdf` | 3.1 s | 1.6 s | 4.7 s | 2 | 6,765 |
| `20-mixed-table-pages.pdf` | 4.7 s | 12.1 s | 16.7 s | 8 | 16,121 |

## Notes

- All 20 jobs were uploaded concurrently to one local service; `max active` is the number of jobs actually running at the same time (the rest waited in the durable queue).
- Accuracy uses the known synthetic fixture content. Page text is compared after whitespace normalization; table cells use exact `raw` strings; OCR CER uses the known `Invoice TOTAL 12345` line. The invoice-occurrence check verifies that hidden OCR text and page OCR are not duplicated.
- Valid PDFs report `partial` because every PDF unit carries conservative reading-order and geometric-table warnings; that is the expected status, not a content failure. The single `document_errors` entry belongs to the intentionally corrupt PDF.
- Page-image capture was disabled for this run so the measurements cover the core extraction path. `elapsed_ms` is excluded when comparing outputs across configurations.
- Synthetic fixtures are not a substitute for real customer scans, handwriting or complex layout corpora.
