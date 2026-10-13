# Cookbook

Copy-paste recipes for the running service (`./run.sh` starts it on `http://127.0.0.1:8787`).
Every recipe works offline; nothing here downloads anything at runtime.

Set once:

```sh
BASE=http://127.0.0.1:8787
ID=$(curl -s -F file=@fixtures/document.docx "$BASE/v1/extractions" | python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])')
```

## Upload and poll

```sh
curl -s -F file=@big.csv -F 'options={"result_format":"text"}' "$BASE/v1/extractions"
curl -s "$BASE/v1/extractions/$ID"                      # state + progress + result_status
curl -s "$BASE/v1/extractions?state=running&limit=20"   # list jobs
curl -s "$BASE/v1/stats"                                # queue, counts, disk
```

`options` is a JSON object and may be empty/omitted. `callback_url` is an optional
multipart field (see webhooks below).

## Get the result

```sh
curl -s "$BASE/v1/extractions/$ID/result" -o result.json          # canonical JSON (or the job's result_format)
curl -s "$BASE/v1/extractions/$ID/result?format=text"             # TSV-style rows
curl -s "$BASE/v1/extractions/$ID/result?format=markdown" | less
curl -s "$BASE/v1/extractions/$ID/result?format=jsonl" | head -3  # one JSON record per row
curl -s "$BASE/v1/extractions/$ID/archive" -o job.zip             # zip of result artifacts (?units=1 adds checkpoints)
```

Large results are streamed; `| head` never downloads the whole body. Preview a chunk
without pulling a 40 MB unit:

```sh
curl -s "$BASE/v1/extractions/$ID/units/u3?from=2000&count=20"    # {"slice": {...}, "unit": {...}}
python3 scripts/preview.py "$ID" --unit u3 --lines 5
```

## Live progress (SSE)

```sh
curl -N "$BASE/v1/extractions/$ID/events"     # one `job` event per change, closes on terminal state
```

## Webhooks (signed, retried)

```sh
# Start the server with a signing secret:
DOC_WEBHOOK_SECRET=dev-secret DOC_REBUILD=1 ./run.sh

# Deliver to your endpoint when the job finishes:
curl -s -F file=@big.csv -F callback_url=https://example.com/hooks/docsmith "$BASE/v1/extractions"
```

Delivery is a POST with the JSON body and these headers:

| Header | Meaning |
|---|---|
| `X-DocSmith-Event` | `job.completed`, `job.failed` or `job.cancelled` |
| `X-DocSmith-Job-Id` | job id |
| `X-DocSmith-Delivery` | unique delivery id (idempotency key) |
| `X-DocSmith-Timestamp` | unix seconds at send time |
| `X-DocSmith-Signature` | `sha256=<hex HMAC-SHA256 of "{timestamp}.{body}">`, present when a secret is set |

Verify before trusting (Python):

```python
import hashlib, hmac
expected = "sha256=" + hmac.new(b"dev-secret", f"{ts}.{raw_body}".encode(), hashlib.sha256).hexdigest()
assert hmac.compare_digest(expected, signature)
```

Failures retry five times with backoff (8 s…64 s); pending deliveries are visible in
`/v1/stats`. Callbacks to loopback/private addresses need `webhook_allow_private: true`
(`DOC_WEBHOOK_ALLOW_PRIVATE=1`), which `run.sh` sets for loopback binds.

## Ship the text result (object storage later)

```sh
# CLI, no server needed; text bytes on stdout:
target/release/doc-extractor extract big.csv --format text --stdout > big.txt
target/release/doc-extractor extract big.csv --format jsonl --stdout | head

# Service: stream the text result straight to a consumer. When you add S3 this is
# `aws s3 cp - s3://bucket/big.txt` — the same pipe, no temp file.
curl -s "$BASE/v1/extractions/$ID/result?format=text" | wc -l
```

## Python client

`clients/python/docsmith.py` is a dependency-free module (also usable as a CLI):

```python
import sys; sys.path.insert(0, "clients/python")
from docsmith import Client

with Client() as client:
    job = client.extract_and_wait("fixtures/digital.pdf", on_progress=lambda j: print(j["progress"]))
    print(job["state"], job["result_status"])
    with open("digital.txt", "wb") as out:
        for chunk in client.stream(job["id"], "text"):
            out.write(chunk)
    client.archive(job["id"], "digital.zip")
    print(client.stats()["jobs"])
```

```sh
python3 clients/python/docsmith.py extract fixtures/digital.pdf --text
python3 clients/python/docsmith.py list --state completed
python3 clients/python/docsmith.py stats
```

## Result viewer

```sh
open "http://127.0.0.1:8787/view/$ID"     # paged tables/text, unit navigation, download links
```

The page is offline HTML (no CDN). For token-protected servers, paste the bearer token in the
header field; it is stored in the browser's localStorage.

## MCP for agent clients

```sh
doc-extractor mcp                       # offline; runs extraction via the same CLI/worker path
doc-extractor mcp --url http://127.0.0.1:8787   # or proxy a running service
```

Claude Code:

```sh
claude mcp add doc-extractor -- doc-extractor mcp
```

Cursor/Claude Desktop config:

```json
{"mcpServers": {"doc-extractor": {"command": "doc-extractor", "args": ["mcp"]}}}
```

Tools: `extract` (path, format, options, max_chars), `capabilities`, and — with `--url` —
`job_status`, `get_result`, `list_jobs`, `stats`.

## Sink command (ship results automatically)

```sh
# Once you have object storage, results upload themselves:
DOC_SINK_CMD='aws s3 cp {result} s3://bucket/results/{id}.{format}' DOC_REBUILD=1 ./run.sh
```

Runs once per completed job with the job directory as cwd; placeholders `{id}`, `{name}`,
`{state}`, `{dir}`, `{result}`, `{format}` (also exported as `DOC_*`). Failures retry three
times and the output lands in `sink-<state>.log`; pending runs show in `/v1/stats` and `/metrics`.

## Metrics

```sh
curl -s http://127.0.0.1:8787/metrics     # Prometheus text: doc_jobs, workers, pending, disk, uptime
```

## Batch upload

```sh
# Up to 32 files per request; each becomes an independent job:
curl -s -F file=@a.pdf -F file=@b.docx -F 'options={"ocr":true}' "$BASE/v1/extractions"
# -> {"jobs":[{"id":"…","cached":false,"filename":"a.pdf"}, …],"total":2}
```

Poll each id as usual; a single-file request keeps the classic `{"id":…,"cached":…}` response.

## HTML export

```sh
curl -s "$BASE/v1/extractions/$ID/result?format=html" -o result.html
# per-job default: -F 'options={"result_format":"html"}'
```

Self-contained HTML (tables, text, asset links) written as `result.html` alongside the other
derived artifacts and offered as a download link in the viewer.

## Cancel, delete, retry

```sh
curl -s -X POST "$BASE/v1/extractions/$ID/cancel"
curl -s -X DELETE "$BASE/v1/extractions/$ID"       # cancels and removes retained files
```

## CLI-only flows

```sh
target/release/doc-extractor extract fixtures/mixed.pdf --ocr --format text --output data/mixed
target/release/doc-extractor capabilities
target/release/doc-extractor openapi | python3 -m json.tool | head
```

## Troubleshooting

- `400 invalid_options` — `options` must be a JSON object; empty is fine, unknown fields are rejected.
- `413` — file exceeds `limits.upload_bytes` (default 32 MiB).
- `429` — `limits.queued_jobs` reached; poll `/v1/stats` or wait for running jobs.
- `409 not_ready` — the result/archive needs `state == completed`; unit checkpoints are available earlier.
- `404 not_found` for `?format=jsonl` — the job predates JSONL artifacts; re-run it or use text/markdown.
