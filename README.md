# Rust document extractor

An offline-first, backend-only Rust extraction service with structured JSON, CLI, HTTP jobs, native parsing, optional OCR, and optional local vision. It returns explicit partial/unsupported results instead of promising arbitrary-document fidelity.

```sh
cargo build --locked
# On this Mac's SDK/linker combination:
scripts/cargo-local.sh build --locked

# Native extraction, no OCR/model required:
target/debug/doc-extractor extract fixtures/workbook.xlsx --output data/example

# Result as text bytes on stdout (pipe-ready, for example to object storage later):
target/debug/doc-extractor extract fixtures/quoted.csv --format text --stdout > quoted.txt

# OCR (provision Tesseract and language packs first):
target/debug/doc-extractor extract fixtures/mixed.pdf --ocr --output data/ocr

# Keep page rasters for a later LLM/vision pass (unreadable pages or every page):
target/debug/doc-extractor extract fixtures/scanned.pdf --page-images failed --output data/vision

# Optional broad adapter (provision the evaluated Xberg 1.3.8 CLI first):
DOC_XBERG=/path/to/xberg target/debug/doc-extractor extract fixtures/digital.pdf --parser xberg --output data/xberg

# HTTP service:
target/debug/doc-extractor serve --bind 127.0.0.1:8787 --data-dir data/service
# Interactive API console (Swagger UI; assets are compiled into the binary, no CDN):
open http://127.0.0.1:8787/docs
curl -F file=@fixtures/document.docx -F 'options={"ocr":false,"vision":false}' http://127.0.0.1:8787/v1/extractions
# Use the returned id:
curl http://127.0.0.1:8787/v1/extractions/JOB_ID/result
# Text instead of canonical JSON for this job: upload with -F 'options={"result_format":"text"}'
# (formats: json | markdown | text | jsonl | html, plus ?format=chunks with chunking enabled).
# RAG chunks: -F 'options={"chunk_chars":2000}' then GET .../result?format=chunks
# Batch: repeat the file field (max 32); each file becomes a job:
curl -F file=@fixtures/digital.pdf -F file=@fixtures/quoted.csv http://127.0.0.1:8787/v1/extractions
# Deploy: docker compose up --build   |   deploy/doc-extractor.service (systemd, see docs/DEPLOYMENT.md)
# Preview rows without downloading a large result (streams the derived export):
python3 scripts/preview.py JOB_ID                 # also: --unit u1, --markdown, --download out.json
# Job listing, stats, live events, zip archive and signed webhooks:
curl http://127.0.0.1:8787/v1/extractions?state=completed
curl http://127.0.0.1:8787/v1/stats
curl -N http://127.0.0.1:8787/v1/extractions/JOB_ID/events
curl -O -J http://127.0.0.1:8787/v1/extractions/JOB_ID/archive
# Result viewer (offline HTML: paged tables/text, download links) and Prometheus metrics:
open http://127.0.0.1:8787/view/JOB_ID
curl -s http://127.0.0.1:8787/metrics
# Python client (dependency-free) and recipes:
python3 clients/python/docsmith.py extract fixtures/digital.pdf --text
# MCP server for agent clients (offline; --url proxies a running service):
target/debug/doc-extractor mcp
# Shell completions:
target/debug/doc-extractor completions zsh > ~/.zfunc/_doc-extractor
# Hindi + English OCR stress fixture and accuracy report (fixtures are committed):
python3 scripts/install-ocr-languages.sh eng hin
python3 scripts/ocr_accuracy.py
```

The result directory contains `result.json`, `result.md`, `units/` and content-addressed `assets/`. With `page_images` enabled it also contains `pages/` (one raster per kept unit) and `pages/manifest.json`, so a later vision/LLM pass can consume the images without parsing the full result; `failed` keeps units whose text is empty, sparse or errored, while `all` keeps every PDF page. Keep JSON for provenance and fidelity; Markdown is a convenience export. Word uses logical sections; spreadsheets use sheets; presentations use source-ordered slides; PDF uses actual pages. PDFs are extracted in 100-page batches with per-page checkpoints (batches run in parallel by default; restart reuses completed pages); CSV/TSV inputs are split into bounded chunk units and support a compact cell form (`--compact-tables`). See [deployment](docs/DEPLOYMENT.md) for large-input sizing.

See [validation evidence](docs/VALIDATION.md), [accuracy report](docs/ACCURACY.md), [concurrent 20-PDF performance report](docs/CONCURRENCY.md), [Hindi + English OCR stress report](docs/hindi-english-ocr.md), [final comprehensive OCR report](docs/final-comprehensive-ocr.md), [cookbook](docs/COOKBOOK.md), [Python client](clients/python/README.md), [capabilities](docs/CAPABILITIES.md), [architecture](docs/ARCHITECTURE.md), [pipeline and concurrency](docs/PIPELINE.md), [research and dependency decisions](docs/RESEARCH.md), [phase acceptance](docs/PHASES.md), and [deployment](docs/DEPLOYMENT.md). These separate implemented behavior from unrun real-world acceptance gates.

## Providers

- PDF: `pdfinfo`, `pdftotext`, `pdfimages`, `pdftoppm` (Poppler).
- OCR: `tesseract`, with requested traineddata provisioned locally. `TESSDATA_PREFIX` can point to your local language directory. OCR is disabled by default. For English + Hindi, run `scripts/install-ocr-languages.sh eng hin` and request `"languages":"eng+hin"`.
- Apple Vision: run `scripts/build-apple-vision.sh`, set `DOC_APPLE_VISION` to the emitted executable, and use `{"ocr":true,"ocr_provider":"apple_vision","languages":"en-US"}`. Query `target/helpers/apple-vision --languages` before selecting other languages. A CPU path avoids observed local GPU runtime failures.
- Xberg: provision the evaluated 1.3.8 CLI plus its bundled libraries, set `DOC_XBERG`, and use `{"parser":"xberg"}` (or `--parser xberg`). The adapter is verified end-to-end and forces `NO_COLOR=1` because the CLI colorizes piped output. The direct adapter retains more spreadsheet formula/merge fidelity on the current fixtures.
- Vision: provision llama.cpp, SmolVLM2 Q4 weights and matching Q8 projector; run `scripts/run-vision.sh`, set `DOC_VLM_URL=http://127.0.0.1:8799`, `DOC_VLM_MODEL=SmolVLM2`, and opt in with `{"vision":true}`. Add `interpret_diagrams:true` to request unverified graph proposals. Nothing is downloaded during extraction. Vision caching requires `DOC_VLM_REVISION` to identify immutable weights/projector/configuration. For lightweight page OCR, `scripts/run-smoldocling.sh` starts SmolDocling-256M (MLX or GGUF) and `{"ocr_provider":"smoldocling"}` parses its DocTags/OTSL tables in pure Rust; if the server is absent, Tesseract is used with a warning.

## Checks

```sh
scripts/cargo-local.sh test --locked
scripts/cargo-local.sh clippy --all-targets -- -D warnings
python3 scripts/e2e.py
# 20-PDF concurrent performance + accuracy report (release build, 3 repetitions):
scripts/cargo-local.sh build --release --locked && python3 scripts/concurrency_report.py
# Pillow is used only to regenerate development fixtures:
python3 scripts/generate-fixtures.py
# Optional measured OCR/model experiments (set DOC_BENCHMARK_OUTPUT to avoid rewriting docs/):
DOC_APPLE_VISION="$PWD/target/helpers/apple-vision" python3 scripts/benchmark.py
# The scheduled/manual Linux benchmark workflow runs the same script on ubuntu-latest:
# .github/workflows/benchmark.yml (results land in the job summary and as an artifact)
```

Synthetic fixtures check contract behavior; they do not establish accuracy across customer documents. Runtime limits and capability probes are configurable. There is no frontend or mandatory Python production service.
