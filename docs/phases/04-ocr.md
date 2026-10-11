# OCR providers and routing

## Tasks delivered

Implement Tesseract TSV, missing-language errors, region/mixed PDF OCR, rotation mapping and optional Apple Vision.

## Decisions

Explicit TSV variable avoids config-file dependencies; sparse fallback and right-angle candidates; no generative model required. Apple CPU recognition follows observed GPU errors.

## Exit evidence

Mixed/hidden OCR duplicate checks; rotation coordinate bounds; clean/noisy/rotated English and Hindi measured against known synthetic text.

## Remaining acceptance

Real noisy/handwritten scans and a complete Paddle/ONNX pipeline have not been evaluated; scores are uncalibrated.

Implementation present; acceptance is limited to the recorded checks. See ../PHASES.md and ../CAPABILITIES.md.
