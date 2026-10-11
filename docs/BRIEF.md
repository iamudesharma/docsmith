# Rust document extraction backend: research and implementation brief
Research date: 11 October 2026. Recommendations are design proposals, not results of local benchmarks.

## 1. Scope and key distinctions
Build a Rust backend and HTTP APIs; no frontend. Native parsing should work with OCR and LLM disabled. Enable OCR independently of optional vision/LLM enrichment.

- Digital text, cells, tables, metadata, embedded assets: parse directly.
- Scanned text and image labels: OCR, which does not require a generative LLM.
- Visual diagrams: preserve the original and extract labels/geometry; understanding relationships requires source structure, geometry heuristics, or optional vision inference.
- No system can guarantee lossless semantic extraction of every arbitrary document. Publish a tested capability matrix and return unsupported, partial, or needs_ocr explicitly.
- “Rust backend” can include audited native libraries through FFI. PDFium and Tesseract are C/C++ dependencies. A strict pure-Rust-only implementation would have different coverage and quality tradeoffs.
- Word documents do not reliably expose stable rendered page boundaries. Preserve sections/paragraphs/tables as logical units; only claim physical pages after an explicit rendering step. CSV and Excel use rows/sheets rather than pages.

## 2. Candidate tools and decisions
| Area | Candidate | Proposed use and limitation |
|---|---|---|
| Broad format coverage | Xberg, active successor to Kreuzberg | Evaluate first behind an adapter. Its maintainers advertise Rust extraction for PDF, Office, markup, email, archives and other formats. Validate each required capability with fixtures; do not equate advertised format support with complete fidelity. |
| PDF text/assets/rendering | pdfium-render + PDFium | Page text, images, objects and rendering. PDF object order is not necessarily reading order; tables and diagram semantics need extra work. Verify PDFium build distribution and concurrency rules. |
| CSV/TSV | csv | Streaming records, delimiter configuration, quoting and flexible rows. Preserve original strings; report encoding and dialect decisions. |
| Excel/ODS | calamine | Reads XLS/XLSX/XLSM/XLSB/ODS and formulas. Treat formula text and cached values separately; do not promise Excel formula recalculation or chart rendering. |
| DOCX/PPTX/ODT etc. | Xberg adapter; ZIP/XML fallback | Preserve paragraphs, lists, tables, hyperlinks, notes and embedded media where tested. Inspect OOXML drawings/relationships for diagram source data. Legacy binary formats require separate fixture coverage. |
| Baseline OCR | Tesseract | Local OCR with language packs, positional outputs such as TSV/hOCR. Suitable portable baseline; noisy scans/handwriting require evaluation. |
| macOS OCR | Apple Vision | Optional macOS-only adapter through Rust Objective-C bindings/FFI. Query supported languages at runtime; do not assume Hindi availability. |
| Later OCR/layout experiment | PaddleOCR / ONNX | Evaluate detection + recognition + preprocessing and postprocessing together. A recognition model alone is not a full OCR pipeline. Confirm ARM64 operators/runtime packaging. No required Python production service. |
| Optional local VLM | SmolVLM2 2.2B quantized via llama.cpp | Small candidate for figure descriptions. Not a guarantee of dense-document or ER-diagram accuracy. Compare against Qwen2.5-VL 3B if resources allow. |
| HTTP service | Axum, Tokio, Serde | Proposed Rust service stack; verify compatible releases during Phase 0. Keep blocking extraction outside async request executors. |

Upstream Xberg/Kreuzberg documentation is in transition and published format counts differ. Pin an evaluated release/commit, confirm crate availability/API/features, and record exactly what that version supports. Prefer reuse over writing dozens of parsers, but isolate the engine so missing structured fields can be supplemented.

## 3. PDFs, images and diagrams
Use page/region-level routing:
1. Inspect native text and page objects.
2. Extract native blocks, geometry, links and assets.
3. Detect image-only or mixed-content regions. Native text presence alone is not proof that a page is fully extracted.
4. OCR only eligible missing-text regions when enabled; avoid duplicating a hidden OCR layer.
5. Preserve figures and vector content with page/bounds provenance.
6. Run diagram interpretation only if explicitly enabled.

Distinguish:
- Extracting embedded image bytes: no OCR or LLM required.
- Reading image text: OCR.
- Extracting chart data: direct chart source data when available; raster charts need specialized interpretation and cannot guarantee exact values.
- Extracting a diagram graph: source parser first; flattened visuals are approximate.

Diagram priorities:
- draw.io XML: parse pages, nodes, edges, labels, groups, geometry and explicit source/target IDs. Support compressed pages via Base64 -> raw DEFLATE -> URI decoding. Detect retained source XML in exported SVG/PNG/PDF where present.
- Mermaid/DOT/PlantUML/DBML: implement explicit supported syntax subsets and return unsupported constructs rather than silently discarding them. Research authoritative grammar/licenses before implementation.
- SVG: extract text, paths, transforms and geometry; arbitrary paths do not inherently encode graph semantics.
- Visio VSDX and Office SmartArt: research separately; preserve assets/source XML until relationship parsing is validated. Legacy VSD deferred initially.
- Flattened vector PDF ER diagrams: geometry/text heuristics can propose entities/edges but mark uncertain directions and cardinalities.
- Raster ER diagrams: OCR labels plus optional VLM interpretation. Keep inferred relationships separate from verified source relationships.

Represent diagrams as nodes, edges, entity attributes, PK/FK evidence, cardinalities, geometry and provenance. Never invent missing foreign keys or cardinalities. Retain original figure assets even when interpretation is disabled.

## 4. M1 Pro, 16 GB RAM
Start with native extraction and Tesseract; optionally benchmark Apple Vision on macOS. No generative model is needed for the baseline.

For optional image understanding, evaluate a Q4 SmolVLM2 2.2B model with its matching vision projector using llama.cpp Metal. The project documents this model and an OpenAI-compatible multimodal server. Use a loopback-only local inference process that the Rust service calls. This keeps the application backend in Rust while using a native inference runtime.

Actual feasibility and quality on this Mac are unmeasured. Model weights are only part of memory: include projector, image tokens, context/KV cache, PDF bitmaps and concurrent jobs. Start with one VLM request and bounded page batches; measure RSS, total memory pressure/swap and latency before raising concurrency. Do not load all pages as high-resolution images simultaneously. Use crops/tiling only with preserved coordinate mappings. A quantized model can still miss labels or hallucinate edges.

## 5. Output contract
Canonical structured JSON is the source of truth; Markdown/plain text are derived exports.
- Document: ID, content hash, MIME, parser/model versions, options, status, warnings and timings.
- Unit: real PDF page / presentation slide / spreadsheet sheet / Word logical section; preserve source order.
- Blocks: text, heading, list, table, figure, diagram, caption, footnote and unknown.
- Provenance: page/unit ID, bounding box when available, sheet/cell range, XML/source reference, native/OCR/inferred method and evidence.
- Tables: grid, merged cells, headers, raw text and typed values, formulas/cached values when available.
- Assets: original bytes or references, dimensions, content hash and placement.
- Diagrams: explicit vs inferred edges and unknown fields.
- Errors: structured document/unit failures; retain successful units.
Do not treat raw model self-reported confidence as calibrated probability.

## 6. Proposed APIs and runtime
- POST /v1/extractions: multipart upload and options; return 202 with job ID.
- GET /v1/extractions/{id}: state, progress, unit failures and warnings.
- GET /v1/extractions/{id}/result: structured JSON or derived Markdown.
- GET /v1/extractions/{id}/units/{unit_id}: unit result.
- GET /v1/extractions/{id}/assets/{asset_id}: extracted asset.
- POST /v1/extractions/{id}/cancel: cancel queued/running work.
- DELETE /v1/extractions/{id}: remove retained result/assets.
- GET /v1/capabilities: actual configured parsers, formats, OCR and model availability.
- GET /health/live and /health/ready.
Generate OpenAPI with precise error/status semantics.

Use adapters for format parsers, OCR providers and optional VLM. Use a bounded worker pool and durable job state/checkpoints (SQLite for a single-machine start; evaluate PostgreSQL for multiple workers). Native parser processes provide crash containment and enforceable timeouts; cancellation of a Rust future does not terminate an FFI call. Persist completed units so restart can resume safely. Cache by content hash plus options/parser/model versions; isolate private results per user/tenant.

Bound upload size, decompressed archive bytes, nesting, page pixels, CPU time, memory, concurrency and output size. Prevent traversal and XML entity expansion; never execute document macros or follow external relationships automatically. Local extraction must work offline after provisioned dependencies/models; no hidden cloud calls or runtime model downloads.

## 7. Phases and acceptance
| Phase | Deliverable | Exit evidence |
|---|---|---|
| 0: Research and spike | Source snapshot, capability matrix, dependency/license decisions, Xberg vs direct-parser spike | Compiling pinned dependencies on available target; recorded results on representative inputs, all limitations stated |
| 1: Foundation | Cargo workspace, JSON schema, parser interfaces, CLI, fixture corpus | Validated schema and provenance; explicit unsupported/password/corrupt errors |
| 2: Native extraction | PDF text, CSV/TSV, Excel/ODS, DOCX; assets retained | No OCR/LLM required; fixture comparisons for text, cells, tables, formulas and page/unit mapping |
| 3: Coverage and layout | PPTX/ODT/RTF/HTML/Markdown/JSON/XML/email/EPUB; legacy adapters by priority; PDF reading order and tables | Version-specific coverage matrix; difficult layouts tested; partial results instead of false success |
| 4: OCR | Tesseract, region routing, optional Apple Vision; Paddle/ONNX research benchmark | Scans, mixed PDFs, English/Hindi, rotated/noisy text evaluated with ground truth; duplicate-text checks |
| 5: Diagrams | draw.io first; selected text-source formats; SVG/PDF preservation and graph heuristics | Source graph accuracy checked; uncertain flattened-diagram results marked inferred |
| 6: Optional vision | Local llama.cpp adapter and model benchmark; optional cloud adapter only if requested | Disabling VLM preserves baseline functionality; memory/latency/accuracy report on actual Mac when available |
| 7: Service and hardening | APIs, durable jobs, cancellation/resume, OpenAPI, macOS/Linux packaging | End-to-end uploads/results/assets, crash/restart/cancel tests, malformed-input limits and deployment instructions |

Implement a minimal API skeleton earlier if needed to validate the pipeline; Phase 7 completes production behavior. Each phase updates its documentation and records measured evidence. Do not claim Mac validation if only Linux is available.

## 8. Sources
Primary sources consulted; re-verify exact pinned versions during Phase 0.
- Xberg source, features, formats, licensing: https://github.com/xberg-io/xberg
- Active successor notice: https://github.com/kreuzberg-dev
- Legacy Rust API (not interchangeable with Xberg): https://docs.kreuzberg.dev/reference/api-rust/
- PDFium Rust wrapper: https://docs.rs/pdfium-render/latest/pdfium_render/
- Calamine: https://github.com/tafia/calamine
- CSV reader: https://docs.rs/csv/latest/csv/struct.ReaderBuilder.html
- Tesseract: https://github.com/tesseract-ocr/tesseract
- Apple Vision OCR: https://developer.apple.com/documentation/vision/vnrecognizetextrequest
- PaddleOCR: https://github.com/PaddlePaddle/PaddleOCR
- draw.io format including embedded source: https://github.com/jgraph/drawio/wiki/File-Format
- draw.io XML reference: https://www.drawio.com/docs/reference/diagram-generation/
- llama.cpp Metal/runtime: https://github.com/ggml-org/llama.cpp
- Multimodal models/server: https://github.com/ggml-org/llama.cpp/blob/master/docs/multimodal.md
- SmolVLM2 model card: https://huggingface.co/HuggingFaceTB/SmolVLM2-2.2B-Instruct
- Quantized candidate: https://huggingface.co/ggml-org/SmolVLM2-2.2B-Instruct-GGUF

## 9. Copyable implementation prompt
Build a backend-only Rust document extraction service with APIs. Read this brief, inspect the repository, and first verify current primary documentation and pinned source versions. Evaluate Xberg (Kreuzberg successor) against direct adapters using pdfium-render, calamine and csv; choose based on fixture fidelity, licenses, native dependencies, maintenance and macOS ARM64/Linux compatibility.

Create docs/RESEARCH.md, CAPABILITIES.md, ARCHITECTURE.md and PHASES.md, plus numbered phase documents covering tasks, decisions, tests and exit criteria. Snapshot relevant official docs/source examples locally under docs/vendor with URL, retrieval date, version/commit, checksum and license; do not download whole sites indiscriminately. Subsequent implementation should use these snapshots until an explicit refresh is needed.

Implement phase by phase: (0) research/spike; (1) structured JSON schema, adapters, CLI and fixtures; (2) native PDF/CSV/Excel/Word extraction; (3) additional formats, reading order and tables; (4) independently optional OCR; (5) source-diagram parsing and figure preservation; (6) optional local vision; (7) production APIs, durable jobs and hardening. Start implementation after the concrete plan and spike; continue through feasible phases without repeated confirmation. Record blocked target-specific validation honestly.

Native extraction must work with OCR and LLM disabled. Preserve embedded images without interpreting them. OCR text does not require a generative LLM. Route OCR per page/region for scans and mixed PDFs. Parse draw.io source graphs first; preserve source/geometry for other diagrams. Keep inferred ER edges/cardinalities separate from verified facts and never fabricate missing fields. Define a tested format matrix rather than promising every file type.

Return page-by-page PDF results, sheet/cell spreadsheet results, slide results, and logical sections for Word unless explicitly rendered. Preserve text, tables, formulas/cached values, assets, metadata, bounds, source references, methods, warnings and partial failures. Use structured JSON as canonical output and derive Markdown.

Use Axum/Tokio, bounded extraction workers, durable checkpoints, cancellation that can terminate isolated native processes, and configurable resource limits. Expose upload/job/progress/result/unit/asset/cancel/delete/capabilities/health APIs and OpenAPI. Default to offline local processing after provisioning. No frontend or mandatory Python service; document all FFI/native dependencies.

For M1 Pro 16 GB, benchmark Tesseract/Apple Vision and optional quantized SmolVLM2 2.2B via llama.cpp Metal; compare a stronger small VLM only if useful. Measure actual memory, swap, latency, OCR error, table fidelity and diagram node/edge accuracy; never claim unrun benchmarks. Test real digital/scanned/mixed/rotated/multicolumn PDFs, merged-cell sheets, Office tables, corrupt/password files, source/vector/raster ER diagrams, large inputs and job recovery. After each phase update the docs with what works, evidence, limitations and next steps.

