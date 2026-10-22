# Hindi + English OCR stress test

Fixture: `final-comprehensive.pdf` · **6 pages** · document status `partial` · extraction 43246 ms.

Ground truth: `fixtures/final-comprehensive.ground-truth.txt`.

## Overall accuracy

| Metric | Result |
|---|---|
| Sequence character error rate | **17.53%** |
| Sequence word error rate | **41.90%** |
| Order-insensitive character recall | **95.94%** |
| Order-insensitive character precision | 98.75% |
| Order-insensitive word recall | **82.45%** |
| Order-insensitive word precision | 72.26% |
| Critical fields found | **41/41** |
| Devanagari characters (OCR / truth) | 1389 / 1453 |
| Latin characters (OCR / truth) | 3189 / 3249 |
| Structured table blocks | **4** |
| Expected table tokens found | 6/6 |
| Unit errors | 0 |

## Per page

| Page | OCR chars | CER | WER | Char recall | Word recall | Blocks | Tables | Devanagari | Latin | Critical fields |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1014 | 12.14% | 63.64% | 92.74% | 80.81% | 19 | 1 | 283 | 406 | 9/9 |
| 2 | 1473 | 15.25% | 42.07% | 96.84% | 84.15% | 33 | 1 | 211 | 713 | 6/6 |
| 3 | 912 | 34.25% | 51.16% | 90.94% | 77.52% | 24 | 0 | 154 | 537 | 7/7 |
| 4 | 544 | 52.11% | 84.34% | 78.69% | 56.63% | 27 | 1 | 100 | 241 | 7/7 |
| 5 | 1583 | 6.56% | 13.28% | 99.04% | 94.61% | 32 | 1 | 420 | 800 | 6/6 |
| 6 | 980 | 6.15% | 41.94% | 97.86% | 79.57% | 24 | 0 | 221 | 492 | 6/6 |

## Critical fields

| Page | Field | Found |
|---:|---|:---:|
| 1 | `उदेश शर्मा` | yes |
| 1 | `7534469` | yes |
| 1 | `PASSED` | yes |
| 1 | `79.33%` | yes |
| 1 | `476 / 600` | yes |
| 1 | `सत्यापित` | yes |
| 1 | `STATE EDUCATION BOARD` | yes |
| 1 | `B2` | yes |
| 1 | `A1` | yes |
| 2 | `INV-2026-0042` | yes |
| 2 | `51,330.00` | yes |
| 2 | `Doc Extractor Supplies` | yes |
| 2 | `GSTIN` | yes |
| 2 | `PAID` | yes |
| 2 | `Fifty-one thousand` | yes |
| 3 | `CUSTOMER` | yes |
| 3 | `INVOICE` | yes |
| 3 | `LINE_ITEM` | yes |
| 3 | `PRODUCT` | yes |
| 3 | `1..*` | yes |
| 3 | `Canonical JSON` | yes |
| 3 | `Data Model` | yes |
| 4 | `Q1` | yes |
| 4 | `Q4` | yes |
| 4 | `96%` | yes |
| 4 | `Language mix` | yes |
| 4 | `Hindi` | yes |
| 4 | `English` | yes |
| 4 | `Summary table` | yes |
| 5 | `E = mc²` | yes |
| 5 | `क्षेत्रफल = πr²` | yes |
| 5 | `H2O + CO2` | yes |
| 5 | `1/2 + 1/3 = 5/6` | yes |
| 5 | `Merged total row` | yes |
| 5 | `Footnotes` | yes |
| 6 | `Compliance Inspection` | yes |
| 6 | `Dr. Anita Verma` | yes |
| 6 | `SATISFACTORY` | yes |
| 6 | `Yes` | yes |
| 6 | `No` | yes |
| 6 | `INSPECTED` | yes |

## Findings

- Most characters were recovered but sequence CER is high: OCR reading order differs from the ground-truth order (diagrams/charts/table columns). Check the order-insensitive recall and per-page text.

## Method

- OCR uses `languages: eng+hin`, page rendering at 150 DPI, automatic rotation and `page_images: all`.
- Character/word rates ignore punctuation, normalize whitespace, map Devanagari digits to ASCII and compare against the committed TXT.
- The PDF is raster-only by design (a scanner simulation), so every character is produced by OCR rather than a PDF text layer.
- Page 3 is rotated 90° to exercise automatic rotation recovery.
