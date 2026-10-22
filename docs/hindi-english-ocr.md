# Hindi + English OCR stress test

Fixture: `hindi-english-complex.pdf` · **3 pages** · document status `partial` · extraction 29418 ms.

Ground truth: `/Volumes/AppleExpanded/Code/doc_ocr_rust/fixtures/hindi-english-complex.ground-truth.txt`.

## Overall accuracy

| Metric | Result |
|---|---|
| Sequence character error rate | **8.98%** |
| Sequence word error rate | **23.00%** |
| Order-insensitive character recall | **95.01%** |
| Order-insensitive character precision | 99.25% |
| Order-insensitive word recall | **79.75%** |
| Order-insensitive word precision | 78.96% |
| Critical fields found | **27/32** |
| Devanagari characters (OCR / truth) | 985 / 1047 |
| Latin characters (OCR / truth) | 1165 / 1180 |
| Structured table blocks | **3** |
| Expected table tokens found | 8/12 |
| Unit errors | 0 |

## Per page

| Page | OCR chars | CER | WER | Char recall | Word recall | Blocks | Tables | Devanagari | Latin | Critical fields |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 875 | 11.61% | 39.34% | 91.19% | 63.93% | 18 | 1 | 255 | 359 | 13/15 |
| 2 | 1401 | 0.18% | 14.59% | 99.91% | 88.11% | 23 | 1 | 536 | 520 | 10/10 |
| 3 | 603 | 23.54% | 18.28% | 87.41% | 83.87% | 11 | 1 | 194 | 286 | 4/7 |

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
| 3 | `Hindi + English` | **no** |
| 3 | `90 degrees` | **no** |
| 3 | `Recovered` | yes |
| 3 | `Registrar` | yes |
| 3 | `TESTED` | **no** |

## Findings

- Only 8/12 expected table tokens were found; small grid text is the weakest region.
- Missing critical fields: 83, A1, Hindi + English, 90 degrees, TESTED

## Method

- OCR uses `languages: eng+hin`, page rendering at 150 DPI, automatic rotation and `page_images: all`.
- Character/word rates ignore punctuation, normalize whitespace, map Devanagari digits to ASCII and compare against the committed TXT.
- The PDF is raster-only by design (a scanner simulation), so every character is produced by OCR rather than a PDF text layer.
- Page 3 is rotated 90° to exercise automatic rotation recovery.
