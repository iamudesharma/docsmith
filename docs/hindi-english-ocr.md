# Hindi + English OCR stress test

Fixture: `hindi-english-complex.pdf` · **3 pages** · document status `partial` · extraction 25358 ms.

Ground truth: `fixtures/hindi-english-complex.ground-truth.txt`.

## Overall accuracy

| Metric | Result |
|---|---|
| Character error rate (letters/digits) | **3.94%** |
| Word error rate | **19.50%** |
| Critical fields found | **29/32** |
| Devanagari characters (OCR / truth) | 1007 / 1047 |
| Latin characters (OCR / truth) | 1195 / 1180 |
| Structured table blocks | **0** |
| Expected table tokens found | 8/12 |
| Unit errors | 0 |

## Per page

| Page | OCR chars | CER | WER | Blocks | Tables | Devanagari | Latin | Critical fields |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 881 | 9.93% | 32.32% | 25 | 0 | 255 | 359 | 13/15 |
| 2 | 1409 | 0.27% | 14.77% | 27 | 0 | 537 | 520 | 10/10 |
| 3 | 670 | 3.65% | 11.11% | 14 | 0 | 215 | 316 | 6/7 |

## Critical fields

| Page | Field | Found |
|---:|---|:---:|
| 1 | `सत्यमेव जयते` | yes |
| 1 | `माध्यमिक शिक्षा परिषद` | yes |
| 1 | `CERTIFICATE-CUM-MARKS SHEET` | yes |
| 1 | `उदेश शर्मा` | yes |
| 1 | `उदेश शर्मा (Udesh Sharma)` | yes |
| 1 | `7534469` | yes |
| 1 | `17 November 2000` | yes |
| 1 | `ST MARY'S C GORAKHPUR` | yes |
| 1 | `PASSED` | yes |
| 1 | `हिंदी` | yes |
| 1 | `English` | yes |
| 1 | `Mathematics` | yes |
| 1 | `83` | **no** |
| 1 | `A1` | **no** |
| 1 | `सत्यापित` | yes |
| 2 | `जिला शिक्षा कार्यालय` | yes |
| 2 | `District Education Office` | yes |
| 2 | `SCHOOL INSPECTION REPORT` | yes |
| 2 | `92 प्रतिशत` | yes |
| 2 | `2,450` | yes |
| 2 | `Key Findings` | yes |
| 2 | `Computer Lab` | yes |
| 2 | `Needs Improvement` | yes |
| 2 | `डॉ. अनीता वर्मा` | yes |
| 2 | `APPROVED` | yes |
| 3 | `COMPLIANCE CERTIFICATE` | yes |
| 3 | `OCR परीक्षण` | yes |
| 3 | `Hindi + English` | yes |
| 3 | `90 degrees` | yes |
| 3 | `Recovered` | yes |
| 3 | `Registrar` | yes |
| 3 | `TESTED` | **no** |

## Findings

- Scanned tables are not promoted to structured `Table` blocks; cells remain linearized OCR text.
- Only 8/12 expected table tokens were found; small grid text is the weakest region.
- Missing critical fields: 83, A1, TESTED

## Method

- OCR uses `languages: eng+hin`, page rendering at 150 DPI, automatic rotation and `page_images: all`.
- Character/word rates ignore punctuation, normalize whitespace, map Devanagari digits to ASCII and compare against the committed TXT.
- The PDF is raster-only by design (a scanner simulation), so every character is produced by OCR rather than a PDF text layer.
- Page 3 is rotated 90° to exercise automatic rotation recovery.
