# AGENTS.md

Offline-first document extraction backend: one Rust crate (`doc-extractor`) with a CLI and an Axum HTTP service. No frontend; the Python scripts are dev/verification tooling only.

## Build and verify

On macOS, run every cargo command through `scripts/cargo-local.sh`: the installed Xcode clang + macOS 27 SDK linker fails on `arm64e.x1` in libSystem.tbd, and the wrapper exports SDK 26.5 per command without touching the machine toolchain.

```sh
scripts/cargo-local.sh fmt --check
scripts/cargo-local.sh clippy --locked --all-targets -- -D warnings
scripts/cargo-local.sh test --locked
scripts/cargo-local.sh build --locked
python3 scripts/e2e.py            # after the debug build
```

CI (`.github/workflows/ci.yml`) runs exactly that order with plain `cargo` on Rust 1.88 (the pinned MSRV); fmt and clippy `-D warnings` are enforced.

- `scripts/e2e.py` starts a real server against `target/debug/doc-extractor` (build first). It stubs a slow `tesseract` on `PATH`, so no OCR install is needed, and it asserts the OpenAPI spec has exactly 9 paths — update it when adding/removing routes. It rewrites `docs/e2e-results.json`.
- OCR/PDF tests in `tests/extraction.rs` silently pass when `tesseract`/Poppler are absent (`providers::available` gate). `brew install poppler tesseract` to actually exercise them.
- `fixtures/` is committed. `scripts/generate-fixtures.py` regenerates it and needs Pillow plus a macOS Arial font path; don't run it on Linux. `scripts/benchmark.py` rewrites `docs/benchmark-results.json`.

## Architecture that changes how you code

- JSON (`model::Document`) is canonical; Markdown is derived. Units are PDF pages / slides / sheets / Word logical sections. Keep `native` vs `ocr` vs `inferred` provenance separate — inferred content (geometric tables, VLM graphs) must never masquerade as native.
- `serve` re-spawns the same executable in hidden `worker` mode per job; each worker is an isolated process group the server kills for cancel/timeout, and it checkpoints every unit (`units/uN.json`, `checkpoint.json`) via atomic rename for crash resume. The CLI `extract` uses the same worker/supervisor path — don't add a second extraction path.
- `parser_fingerprint()` in `extract.rs` hashes `extract.rs`, `diagrams.rs`, `providers.rs` and `Cargo.lock`, so cache identity changes when any of them change.
- `capabilities`, `schema`, and `openapi` subcommands print the live contracts; the service exposes `/` (landing), `/v1/capabilities`, `/v1/extractions` (list + upload; batch = repeat the `file` field, max 32; with `/result`, `/units/{id}` row slices, `/assets/{id}`, `/events` SSE, `/archive`, `/cancel`, DELETE), `/v1/stats`, `/metrics` (Prometheus), `/view/{id}` (offline viewer), `/openapi.json`, `/docs` (offline Swagger UI), `/health/live`, `/health/ready`. Webhooks (`callback_url`) are HMAC-signed when `webhook_secret` is set and retried with backoff; `sink_command` runs a shell command per completed job and the `s3` block uploads artifacts natively with SigV4 (`sink_command` wins when both are set); `watch_dir` ingests files dropped on disk. `mcp` is a stdio MCP tool server (offline, or proxy a service with `--url`). `DOC_LOG_JSON=1` switches logs to JSON lines.

## Optional providers (all off by default; extraction never downloads)

- OCR: `tesseract` + traineddata; `TESSDATA_PREFIX` selects an isolated language dir; opt in per request with `{"ocr":true}`.
- Apple Vision (macOS only): `scripts/build-apple-vision.sh`, then `DOC_APPLE_VISION=/abs/path`; `{"ocr_provider":"apple_vision"}`.
- Xberg: `DOC_XBERG=/abs/path/to/xberg` (expects the evaluated 1.3.8 CLI); `{"parser":"xberg"}`; its mapping is deliberately partial.
- VLM: `scripts/run-vision.sh` expects `models/*.gguf` (gitignored, provisioned separately); then `DOC_VLM_URL=http://127.0.0.1:8799`, `DOC_VLM_MODEL=SmolVLM2`, and `DOC_VLM_REVISION` (required for VLM caching).
- Service: `DOC_TOKENS` maps bearer tokens to tenants and is required for non-loopback binds; `DOC_BIND`/`DOC_DATA_DIR` override defaults; runtime state lives in `data/` (gitignored).

## Docs

`docs/ARCHITECTURE.md` and `docs/DEPLOYMENT.md` are the current design/ops references; `docs/BRIEF.md` is the original brief. README links to `docs/CAPABILITIES.md`, `docs/RESEARCH.md` and `docs/PHASES.md`, which exist alongside the numbered `docs/phases/` documents. `docs/vendor/` holds dated upstream snapshots with checksums (`manifest.json`); prefer them over re-fetching vendor docs. Validation evidence lives in `docs/VALIDATION.md`/`VALIDATION.json`, `docs/e2e-results.json`, `docs/benchmark-results.json` and `docs/xberg-adapter-check.json`.
