# Native document extraction

## Tasks delivered

Implement native PDF, CSV/TSV, Excel/ODS and DOCX, with OCR and LLM independently disabled.

## Decisions

Direct Calamine values/formulas, original workbook and merge source; actual PDF pages and original vectors; logical Word sections and media.

## Exit evidence

Leading zeros, multiline CSV, typed values, A3 formula/cache, merges, sections/assets, PDF geometry and password errors. Hand-built BIFF8 XLS and macro-enabled XLSM fixtures assert typed formula/cached values and that VBA source is never extracted or executed.

## Remaining acceptance

Complex grid/reading order, floating Word object geometry, date-formatted XLS, XLSB and large-workbook varieties need real fixtures.

Implementation present; acceptance is limited to the recorded checks. See ../PHASES.md and ../CAPABILITIES.md.
