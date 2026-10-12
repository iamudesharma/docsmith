# Architecture

Backend only. Rust 1.88, Axum 0.8, Tokio, Serde, SQLite WAL. Cargo.lock is the dependency pin. JSON is canonical; Markdown/text are derived. Schema is emitted by `doc-extractor schema`; OpenAPI by `doc-extractor openapi` and `/openapi.json`.

## Pipeline

Multipart is streamed to a private job directory. SQLite stores the tenant, content/options/parser identity, request, queue state and timestamps. A bounded dispatcher starts the same executable in worker mode. Each worker owns a process group; Poppler, Tesseract, Apple Vision and Xberg run inside it. Native extraction never runs on an async HTTP executor. SIGKILL of the process group cancels native work and removes descendants. CPU/file rlimits, wall deadlines, process-tree RSS sampling and scratch quotas bound execution. Linux additionally uses RLIMIT_AS; macOS RSS is sampled, so it is not an instantaneous hard allocation limit. These are resource controls, not a kernel security sandbox.

Workers atomically write per-unit JSON; the whole-document checkpoint and progress manifests are refreshed on a 100-unit cadence while `units/uN.json` stays current for every unit. PDF resume identity lives in `identity.json` and per-asset sidecars, so a restarted PDF reuses completed pages without invoking Poppler at all (text and image extraction run in 100-page batches). Other adapters restart deterministically from retained input and atomically replace their completed logical units. No result is considered available until the final worker output exists and the process exits successfully. Extraction failures may have a completed job with document status `failed`, `unsupported`, `partial` or `needs_ocr`; job state describes execution, document status describes coverage.

The server takes an advisory lock on its data directory. Restart reaps registered orphan worker groups after checking their command identity, recovers `running` jobs to `queued`, and finishes interrupted deletions. Cancellation is idempotent and never overwritten by late worker completion. Deletion waits for workers to stop before deleting data. Closing the server kills workers and leaves jobs resumable on restart.

## Adapters and provenance

Direct parsers are authoritative for spreadsheet types/formulas and Office/source diagram units. The optional pinned Xberg CLI is a process adapter, selected through `parser: "xberg"`; its entire structured output is retained as an asset. It always runs with discovery, caches, layout models, OCR and model features disabled. Its canonical mapping is explicitly partial.

Poppler yields actual PDF pages, line geometry and embedded images. Geometric table proposals retain raw text and are marked inferred. OCR runs on embedded images (including mixed pages); image-only pages can be rendered one at a time. Missing text routing and duplicate suppression are conservative; image placement is not reconstructed. Original PDF bytes retain vector drawings. Source draw.io relationships are verified source facts. Flattened PDF label boxes and VLM graph output are separate inferred blocks. Unknown keys and cardinalities stay unknown.

Tesseract emits TSV; right-angle rotation candidates use relative OCR scores for selection, with inverse coordinate transforms to original-image pixels. Scores are not calibrated probabilities. Apple Vision is provisioned as a native Swift helper and emits normalized top-left geometry. Supported languages are queried at runtime. Vision language codes differ from Tesseract codes.

The optional llama.cpp adapter accepts literal loopback HTTP addresses only, disables redirects/proxies, uses finite requests and capped response bytes, and serializes model calls with a cross-worker file lock. Model output never controls document processing or executes instructions. No cloud provider is implemented. No document request downloads a model.

## Isolation and retention

Without tokens the service binds to loopback and uses a single local tenant. On other bind addresses a token-to-tenant map is required. The tenant comes from the bearer token, not caller-supplied tenant headers. Every job/result/unit/asset/cancel/delete endpoint checks ownership. Cache entries are tenant-scoped and keyed by file hash, filename, normalized options, parser source/Cargo.lock fingerprint and provider identity. VLM caching requires an explicit immutable `DOC_VLM_REVISION`.

Upload and archive limits are checked independently. ZIP paths, symlinks, duplicate entries, archive size/entry count and email nesting are validated. XML DTDs/entities are rejected. External relationships/HTML URLs are retained but never fetched. Macros are never executed. This version does not recursively extract archive attachments. Assets are addressed by hashes and served as attachments with `nosniff`; source labels never become filesystem paths.

Retention is explicit through DELETE, with an opt-in per-tenant `disk_bytes` quota for new storage and an opt-in `retention_seconds` sweep for terminal jobs. Multi-server workers, PostgreSQL, per-tenant instantaneous memory accounting, OS sandbox profiles and long-soak tests remain deployment work. Use an OS/container limit for an instantaneous memory boundary.
