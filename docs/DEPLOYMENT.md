# Build, provision and run

## macOS ARM64

Rust 1.88+ and a compatible Xcode/Command Line Tools SDK are required. The observed host has Xcode's clang 17 plus SDK 27; their linker combination fails on the new `arm64e.x1` architecture in libSystem.tbd. `scripts/cargo-local.sh` selects installed SDK 26.5 per command and was used for local builds/tests. It does not change xcode-select, shell files or system SDKs.

Provision native programs once, e.g. `brew install poppler tesseract`. These downloads are provisioning operations, never document-request behavior. English language data must be present. `TESSDATA_PREFIX=/absolute/local/tessdata` can select an isolated installation; provision `eng.traineddata`, `osd.traineddata` and `hin.traineddata` as needed. Training data licensing and checksums are listed in the vendor manifest.

Build Apple Vision with `scripts/build-apple-vision.sh`, then export its absolute executable path as `DOC_APPLE_VISION`. The helper queries supported languages and rejects unsupported selections. It uses a CPU request and a decoded CGImage, avoiding observed local Vision GPU execution errors. Apple SDK/platform terms apply; this helper is unavailable on Linux.

Xberg's evaluated release is 1.3.8. Its Rust crate requires Rust 1.92; the direct backend builds on 1.88. The 1.3.8 ARM64 CLI release was evaluated as a standalone process and the adapter path is verified end-to-end (see docs/xberg-adapter-check.json); set `DOC_XBERG=/absolute/path/to/xberg` while keeping its bundled dylibs together. The adapter exports `NO_COLOR=1` and parses the CLI version tolerantly because upstream colorizes stdout even when piped. It is optional. The upstream bundle includes third-party libraries with separate licensing obligations; do not assume the MIT engine license covers all bundled libraries. The downloaded bundle is ignored under target/, and attribution files are retained under docs/vendor/.

## Local vision

Use a provisioned llama.cpp server. The measured installation reports build 11146, commit 7fe450e19. Use `scripts/run-vision.sh` after separately placing the GGUF files in models/:

- repository: `ggml-org/SmolVLM2-2.2B-Instruct-GGUF`
- evaluated revision: `1bc3c9f74ceafd4c8d4411cc9cf188bba3798f91`
- `SmolVLM2-2.2B-Instruct-Q4_K_M.gguf`
- matching `mmproj-SmolVLM2-2.2B-Instruct-Q8_0.gguf`

The helper script does not download files. Weights are ignored by Git. Set `DOC_VLM_URL=http://127.0.0.1:8799`, `DOC_VLM_MODEL=SmolVLM2`, and an immutable `DOC_VLM_REVISION` (weights/projector/context/quantization identity) if enabling VLM result caching. A server-owned file lock admits one model request at a time across workers. Request cancellation closes the HTTP call; the external llama.cpp server owns its inference cancellation behavior. Server-side memory is outside the extraction worker memory quota.

The actual test host is **MacBookAir10,1, M1, 8 GB**, not the brief's M1 Pro 16 GB. See measured RSS and swap snapshots in benchmark-results.json. GGUF size, worker RSS and model-server RSS are distinct; Metal allocations and compressed memory need whole-system monitoring. Do not increase concurrency from this small synthetic experiment.

## Configuration and authentication

```json
{
  "bind": "127.0.0.1:8787",
  "data_dir": "data/service",
  "tokens": {"replace-with-secret-token": "tenant-a"},
  "limits": {
    "workers": 2,
    "queued_jobs": 100,
    "upload_bytes": 33554432,
    "archive_bytes": 134217728,
    "archive_entries": 4096,
    "units": 20000,
    "page_pixels": 20000000,
    "output_bytes": 536870912,
    "timeout_seconds": 1800,
    "memory_bytes": 2147483648,
    "pdf_workers": 4,
    "ocr_threads": 4,
    "stream_bytes": 16777216,
    "disk_bytes": 0,
    "retention_seconds": 0
  }
}
```

## Large inputs

Measured on the recorded 8 GB M1 (synthetic inputs; see perf-large-inputs.json): a 3000-page text PDF completes in ~21 s (release, four batch workers) / ~30 s (one worker) / ~62 s (debug) with 3000 page units and no errors, and restarting an unchanged 500-page job reuses every checkpointed page in under a second. A 700k-row, 30 MB CSV completes in ~3.3 s (release). Inputs above `limits.stream_bytes` (default 16 MiB) are extracted in streamed mode: chunk blocks are released once their `units/uN.json` is written, and `result.json`/`result.md`/`result.txt` are assembled by streaming those unit files, so live heap stays near one chunk (measured 107 MB peak versus 915 MB buffered for the same 700k-row file; final live heap 0). Note that macOS `RSS`/footprint overstate steady-state usage because the system allocator keeps freed pages resident — use the Linux CI benchmark or the live-heap measurement for capacity planning. Consequences for very large jobs:

- `upload_bytes` must cover the file (default 32 MiB); `timeout_seconds` (default 1800 s) must cover the whole job.
- `pdf_workers` runs independent 100-page batches concurrently (default 4, clamped to 1..8); batches are handed out dynamically, so slow image-heavy batches do not idle other workers. `ocr_threads` caps concurrent Tesseract processes (default 4) and is enforced machine-wide across worker processes through flock slot files in `<data_dir>/ocr-slots`, so two concurrent scanned jobs share the same OCR budget. Results are merged in page order and are identical for any worker count. On a page-count mismatch from Poppler, text is re-fetched per page rather than misattributed, and a page whose render/OCR fails only marks that page partial.
- `compact_tables: true` (request option, `--compact-tables` on the CLI) omits redundant cell fields for delimited files: ~4x input size instead of ~12x (203 MB -> 68.5 MB for the 400k-row sample) and roughly half the peak memory. A missing `value` means "the raw string".
- `result_format: "text"` (request option, `--format text` on the CLI) makes the derived plain text the default export of `GET .../result` — for the measured 27 MB / 100k-row CSV the text artifact is 25 MB versus 155 MB canonical JSON, while JSON stays available with `?format=json`. On the CLI, `--format text --stdout` streams the artifact as bytes so it can be piped to object storage or another consumer; all artifacts are still written to the job directory.
- CSV result JSON is roughly 12x the input for tiny cells in the default full mode; size `disk_bytes`/`output_bytes` and backup space accordingly. The result write is streamed with a size cap, so an oversized job fails with `limit_exceeded: output bytes` instead of a truncated file.
- Peak memory scales with total document size (about 0.9 GB per 700k rows of the measured shape; compact mode reduces this). Raise `memory_bytes` on a larger host, or split inputs; a fully streamed table mode is future work.
- Per-unit files are always durable; the whole-document checkpoint and progress manifests are refreshed every 100 units, so coarse progress can lag for very large jobs while `units/uN.json` remains current.

`disk_bytes` is a per-tenant retained-bytes quota (0 disables it): uploads that would exceed it are rejected with 507 `quota_exceeded`, while cache hits that store nothing are still served. `retention_seconds` enables a sweep that removes terminal jobs older than the window (0 disables it); the sweep runs every `min(retention_seconds, 15)` seconds and never touches jobs whose workers are still active. Explicit DELETE remains available regardless.

`doc-extractor serve --config configuration.json` reads this file. Unspecified fields use defaults. `DOC_TOKENS` can provide the token-to-tenant JSON mapping through the environment. Keep secrets outside source control; protect the data directory with service-user filesystem permissions. All resource endpoints enforce the mapped tenant. Liveness, readiness, capabilities, the `/docs` Swagger UI and OpenAPI expose no document content and do not require authentication. Readiness confirms the job database; missing optional parsers are reported separately in capabilities.

Webhooks are opt-in per upload (`callback_url` multipart field). Deliveries are POSTed once the job reaches `completed`, `failed` or `cancelled`, with `X-DocSmith-Event`, `X-DocSmith-Job-Id`, `X-DocSmith-Delivery` (idempotency key), `X-DocSmith-Timestamp` and — when `webhook_secret` is set (`DOC_WEBHOOK_SECRET`) — `X-DocSmith-Signature: sha256=<HMAC-SHA256 of "{timestamp}.{body}">`. Failures retry five times with 8–64 s backoff; pending deliveries are counted in `/v1/stats`. Callback URLs must be http(s); loopback and private addresses are refused unless `webhook_allow_private` (`DOC_WEBHOOK_ALLOW_PRIVATE=1`) is set, which `run.sh` does automatically for loopback binds.

`sink_command` (`DOC_SINK_CMD`) runs a shell command once per completed job with the job directory as cwd. Placeholders `{id}`, `{name}`, `{state}`, `{dir}`, `{result}` and `{format}` expand to job values (also exported as `DOC_*` variables), so results can be shipped without an SDK or an object-storage dependency: `DOC_SINK_CMD='aws s3 cp {result} s3://bucket/results/{id}.{format}'`. Failures retry three times; output is appended to `sink-<state>.log` in the job directory and pending runs are counted in `/v1/stats` and `/metrics`. Treat the command as operator-trusted configuration (it runs with the service user's privileges).

MCP clients (Claude Code, Cursor, LM Studio, ...) can use the extractor as a local tool server: `doc-extractor mcp` runs extraction offline through the same CLI/worker path, and `doc-extractor mcp --url http://127.0.0.1:8787` proxies a running service (which also enables `job_status`, `get_result`, `list_jobs` and `stats`). Tool output is truncated to `max_chars` (default 20 000) so large results never flood an agent context. The offline result viewer at `/view/{id}` renders paged tables and text with download links (bearer token supported in-page), and `/metrics` exposes Prometheus gauges (jobs by state, workers, pending webhooks/sinks, disk, uptime).

The server defaults to loopback. A non-loopback bind requires bearer tokens. Place a TLS reverse proxy in front for network deployment and apply connection/body/rate limits there. Do not share one SQLite directory across multiple servers. Run as an unprivileged service user. A container/cgroup or OS sandbox adds a strict memory and filesystem/network security boundary; process isolation here contains parser crashes but is not a security sandbox.

Job state, input, checkpoints, logs and assets are retained until DELETE, expiry of `retention_seconds`, or (optionally) the per-tenant `disk_bytes` quota limiting new storage. Back up the SQLite database and its job directories as one consistent snapshot while stopped, or use SQLite's backup API and matching immutable job snapshots. Restore into a private stopped directory, then restart. Interrupted jobs requeue and PDF checkpoints can resume completed pages; other formats safely reparse retained input. Long-soak, backup restoration and multi-host deployments have not been validated.

## Linux

The Dockerfile and CI workflow install Poppler and English/Hindi Tesseract data. Apple Vision is unavailable. Linux adds RLIMIT_AS to the sampled process-tree RSS guard. The image includes GPL-licensed system tools through the distribution package manager; comply with redistribution obligations. Local macOS tests do not establish Linux/container compatibility.

`.github/workflows/benchmark.yml` runs the synthetic benchmark on `ubuntu-latest` (manual `workflow_dispatch` or the weekly schedule) with the same Poppler/Tesseract packages, asserts every case produced a result, renders a table into the job summary and uploads `benchmark-results.json` as an artifact. `DOC_BENCHMARK_OUTPUT` overrides the output path so CI runs never rewrite `docs/benchmark-results.json`, and `DOC_BENCHMARK_STRICT=1` turns a missing result or nonzero extractor exit into a job failure. Runner timings are shared-VM smoke signals, not target-hardware performance evidence; the optional VLM and Apple Vision providers are not exercised there.

## API status semantics

POST `/v1/extractions` uses multipart `file` plus JSON-encoded `options` (empty means defaults) and an optional `callback_url`; success returns 202 `{id,cached}`. Poll GET job for `queued`, `running`, `completed`, `failed` or `cancelled`, list with `GET /v1/extractions?state=&limit=&offset=`, or stream changes with `GET /v1/extractions/{id}/events` (server-sent events, closes on terminal state). `GET /v1/stats` reports per-state counts, active/slot workers, pending webhook deliveries and jobs-directory disk usage. GET result returns 409 until the job completes. It serves the job's `result_format` option (`json` canonical, `markdown`, `text` or `jsonl` — one JSON record per row), and `?format=json|markdown|text|jsonl` overrides it per request. Unit checkpoints can be retrieved earlier, and `?from=&count=` returns a `{slice, unit}` table preview instead of a full multi-megabyte unit. `GET /v1/extractions/{id}/archive` streams a zip of the result artifacts (`?units=1` includes checkpoints). A completed job can have partial/unsupported/failed document coverage. GET assets returns original bytes as an attachment. POST cancel is idempotent. DELETE cancels, waits for worker termination, removes retained files/DB row and returns 204; if termination is still pending it returns retryable 409. Error responses have `{error:{code,message}}`. Limits produce 413 for upload bytes and 429 for queue saturation; document/unit failures retain successful units. The Swagger UI at `/docs` renders the same live contract (`/docs/openapi.json`) with try-it-out and a bearer-token `Authorize` field when tokens are configured; its assets are compiled into the binary, so it works offline.
