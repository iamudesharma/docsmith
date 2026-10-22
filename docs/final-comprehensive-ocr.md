# Hindi + English OCR stress test

Fixture: `final-comprehensive.pdf` · **6 pages** · document status `partial` · extraction 57610 ms.

Ground truth: `fixtures/final-comprehensive.ground-truth.txt`.

## Overall accuracy

| Metric | Result |
|---|---|
| Sequence character error rate | **21.24%** |
| Sequence word error rate | **47.71%** |
| Order-insensitive character recall | **96.46%** |
| Order-insensitive character precision | 99.03% |
| Order-insensitive word recall | **82.20%** |
| Order-insensitive word precision | 71.89% |
| Critical fields found | **32/41** |
| Devanagari characters (OCR / truth) | 1428 / 1453 |
| Latin characters (OCR / truth) | 3150 / 3249 |
| Structured table blocks | **4** |
| Expected table tokens found | 6/6 |
| Unit errors | 0 |

## Per page

| Page | OCR chars | CER | WER | Char recall | Word recall | Blocks | Tables | Devanagari | Latin | Critical fields |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1076 | 19.65% | 78.79% | 98.87% | 82.83% | 19 | 1 | 319 | 396 | 9/9 |
| 2 | 1457 | 15.34% | 41.46% | 96.32% | 84.15% | 31 | 1 | 205 | 706 | 5/6 |
| 3 | 920 | 48.56% | 63.57% | 92.52% | 79.84% | 30 | 0 | 153 | 551 | 6/7 |
| 4 | 543 | 50.00% | 81.93% | 79.75% | 59.04% | 26 | 1 | 104 | 236 | 5/7 |
| 5 | 1570 | 9.84% | 21.58% | 97.84% | 90.87% | 34 | 1 | 426 | 778 | 2/6 |
| 6 | 970 | 4.95% | 40.86% | 97.59% | 79.57% | 23 | 0 | 221 | 483 | 5/6 |

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
| 2 | `PAID` | **no** |
| 2 | `Fifty-one thousand` | yes |
| 3 | `CUSTOMER` | yes |
| 3 | `INVOICE` | yes |
| 3 | `LINE_ITEM` | yes |
| 3 | `PRODUCT` | yes |
| 3 | `1..*` | **no** |
| 3 | `Canonical JSON` | yes |
| 3 | `Data Model` | yes |
| 4 | `Q1` | **no** |
| 4 | `Q4` | yes |
| 4 | `96% accuracy` | **no** |
| 4 | `Language mix` | yes |
| 4 | `Hindi` | yes |
| 4 | `English` | yes |
| 4 | `Summary table` | yes |
| 5 | `E = mc²` | **no** |
| 5 | `क्षेत्रफल = πr²` | **no** |
| 5 | `H2O + CO2` | **no** |
| 5 | `1/2 + 1/3 = 5/6` | **no** |
| 5 | `Merged total row` | yes |
| 5 | `Footnotes` | yes |
| 6 | `Compliance Inspection` | yes |
| 6 | `Dr. Anita Verma` | yes |
| 6 | `SATISFACTORY` | yes |
| 6 | `Yes` | yes |
| 6 | `No` | yes |
| 6 | `INSPECTED` | **no** |

## Findings

- Most characters were recovered but sequence CER is high: OCR reading order differs from the ground-truth order (diagrams/charts/table columns). Check the order-insensitive recall and per-page text.
- Missing critical fields: PAID, 1..*, Q1, 96% accuracy, E = mc², क्षेत्रफल = πr², H2O + CO2, 1/2 + 1/3 = 5/6, INSPECTED

## Method

- OCR uses `languages: eng+hin`, page rendering at 150 DPI, automatic rotation and `page_images: all`.
- Character/word rates ignore punctuation, normalize whitespace, map Devanagari digits to ASCII and compare against the committed TXT.
- The PDF is raster-only by design (a scanner simulation), so every character is produced by OCR rather than a PDF text layer.
- Page 3 is rotated 90° to exercise automatic rotation recovery.
