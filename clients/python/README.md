# Python client

`docsmith.py` is a single-file, dependency-free client for the doc-extractor HTTP API
(Python 3.9+; uses `urllib` only).

```python
import sys; sys.path.insert(0, "clients/python")
from docsmith import Client, ApiError

with Client("http://127.0.0.1:8787") as client:      # token= or DOC_TOKEN for protected servers
    job = client.extract_and_wait("report.pdf", options={"ocr": True}, on_progress=print)
    print(job["state"], client.text(job["id"])[:200])
```

Highlights:

- `extract` / `extract_and_wait` — upload one document, optionally with `options` and `callback_url`
- `stream` / `download` — result bytes chunk by chunk (no full buffering)
- `text` / `markdown` / `jsonl` / `document` — the four representations
- `units(job, "u3", rows=20)` — cheap `{slice, unit}` preview of a large chunk
- `archive` — zip of result artifacts (`units=True` adds checkpoints)
- `list` / `stats` / `status` / `cancel` / `delete`
- CLI: `python3 docsmith.py extract report.pdf --text`, `list`, `stats`

Errors raise `ApiError` with `.status` and the decoded `.payload`.
