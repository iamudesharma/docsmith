# Extraction accuracy — 2026-10-13 21:03

Host `MacBookAir10,1`, binary `0870c8d788707585…`, Tesseract languages: eng, osd, snum.
Synthetic fixtures with known content (see `scripts/generate-fixtures.py`); script: `scripts/accuracy.py`.

| Metric | Result |
|---|---|
| Content/structure/integrity checks | **25/25** |
| Table cells exact (CSV/TSV/XLSX/XLS/XLSM/ODS/PDF) | **29/29** (100.0%) |
| PDF page texts exact | **138/138** (100.0%) |
| OCR mean character error rate | **0.0000** over 5 raster cases |

## OCR detail

| Case | CER | WER | Extracted |
|---|---|---|---|
| OCR clean raster | 0.0000 | 0.0000 | `Invoice TOTAL 12345` |
| OCR noisy raster | 0.0000 | 0.0000 | `Invoice TOTAL 12345` |
| OCR rotated raster (90° probe) | 0.0000 | 0.0000 | `Invoice TOTAL 12345` |
| OCR image-only PDF page | 0.0000 | 0.0000 | `Invoice TOTAL 12345` |
| Mixed page: native + OCR | 0.0000 | 0.0000 | `Invoice TOTAL 12345` |

## Measured (informational, inferred content)

- OCR raster ER figure entity text (measured, informational): recall 0.50 — found ['orders'] of ['users', 'orders']

## Skipped

- OCR Hindi raster (skipped without hin traineddata)

## Limits

- Synthetic fixtures with known ground truth; real customer scans/handwriting remain unrun.
- OCR is measured in English only on this host; the Hindi fixture is skipped unless `hin` traineddata is installed.
- Geometric PDF tables and raster/vector diagram graphs are proposals (inferred), not claimed fidelity - the ER raster entity recall line records how much text OCR recovers, not graph correctness.
- Table-cell accuracy compares exact cell `raw` strings (leading zeros and multiline content preserved).
