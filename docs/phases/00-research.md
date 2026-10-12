# Research and spike

## Tasks delivered

Inspect empty repository and installed tools; snapshot official documentation; compare broad engine and direct parser output.

## Decisions

Use direct adapters on Rust 1.88; isolate optional Xberg 1.3.8 CLI because the crate requires 1.92. Poppler is provisioned; PDFium is not.

## Exit evidence

Cargo build and fixture-specific Xberg snapshots, later followed by end-to-end adapter verification after the ANSI version-output bug was found. Exact compile/runtime versions and licensing notes in RESEARCH.md; adapter checks in xberg-adapter-check.json.

## Remaining acceptance

Linked Xberg/PDFium and Linux tests remain unrun; advertised formats are not evidence.

Implementation present; acceptance is limited to the recorded checks. See ../PHASES.md and ../CAPABILITIES.md.
