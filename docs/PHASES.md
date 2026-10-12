# Phase execution and acceptance

All eight phases have implementation artifacts. This does **not** mean all research-brief acceptance gates have passed. The backend is a tested local foundation; optional experiments and real-world fidelity gates are explicit below.

| Phase | Delivered | Acceptance remaining |
|---|---|---|
| 0 | Official snapshots/checksums, locked compiling direct dependencies, seven-fixture Xberg process spike, adapter-level Xberg verification, dependency/license decisions | Rust 1.92 linked Xberg/PDFium comparison; Linux execution |
| 1 | Cargo workspace, canonical types/JSON schema, adapter boundary, isolated CLI, deterministic fixture generator | Wider real-document oracle corpus |
| 2 | PDF/CSV/Excel/Word text, tables/formulas/assets and source unit mapping; BIFF8 XLS and macro-enabled XLSM fixtures | Complex sheets/layout/date formats, XLSB and large-workbook corpus; exact rendered placements |
| 3 | PPTX/ODT/ODP/RTF/HTML/Markdown/JSON/XML/email/EPUB/Visio source, chart caches, inferred PDF table proposals | Exhaustive rich format fidelity and complex table/layout topology |
| 4 | Independently optional Tesseract/Apple Vision, mixed regions, duplicate checks, rotations, measured English/Hindi synthetic OCR | Real scanner/handwriting corpus; Paddle/ONNX full pipeline |
| 5 | draw.io and retained source, source-language subsets, explicit keys/cardinalities, SVG/source/vector preservation and inferred label geometry | Complex syntax; validated Visio/SmartArt semantics; flattened connector recovery |
| 6 | Offline loopback-only llama.cpp adapter, serialized requests, optional constrained inferred graphs, small-model benchmark | Stronger-model comparison if useful; reliable dense/table/ER accuracy; M1 Pro 16 GB |
| 7 | Axum APIs, SQLite queue/checkpoints, tenant/cache ownership, process cancellation/timeouts/resource controls, opt-in per-tenant disk quota and retention sweep, OpenAPI, Docker/CI/deployment files | Linux CI/container execution, long soak, kernel sandbox, backup restore and network deployment review |

Numbered phase documents list decisions, tests and limits. The latest tests are reproducible through README.md. `e2e-results.json` records live API checks; `benchmark-results.json` records measured runs, fixture identity, sampled process-tree RSS and swap snapshots. Small synthetic acceptance is not universal fidelity or production deployment proof.
