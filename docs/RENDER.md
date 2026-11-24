# Deploying on Render (GitHub → Render)

This is the prepared path for a shared testing backend: push to GitHub, Render builds the image
from `render.yaml` + `Dockerfile.render`, and every later push to the branch is redeployed
automatically. OCRia-nano-1 is the default OCR backend and the model weights are baked into the
image, so the running service never downloads a model.

## What is in the image and how it is limited

| Limit | Free profile (`render.json`) | 2 GB profile (`render-2gb.json`) |
|---|---:|---:|
| Active jobs | 1 | 2 |
| Concurrent OCR calls across jobs | 1 | 2 |
| Parallel PDF batches per job | 1 | 1 |
| Process-tree RSS budget per worker | 416 MiB | 640 MiB |
| Linux virtual address space per process | 2 GiB | 2 GiB |

Both profiles use LiteParse 2.21.1 with Tesseract fallback, allow eight queued and active jobs
combined, accept up to 32 MiB per input, and allow up to 600 seconds per job. The supervisor
samples worker process-tree RSS and fails over-budget jobs with `limit_exceeded`. The separate
virtual address-space limit leaves room for LiteParse and ONNX mappings. Container memory also
includes the API process, filesystem cache, and allocations between RSS samples.

## 1. Push the repository to GitHub

```sh
git remote add origin git@github.com:<you>/<repo>.git
git push -u origin HEAD
```

Keep `render.yaml` and `Dockerfile.render` at the repository root and commit both profiles under
`deploy/` with them.

## 2. Connect Render once

1. Render Dashboard → **New → Blueprint**.
2. Connect GitHub and pick the repository (the branch that holds `render.yaml`).
3. Render shows the `doc-extractor` service from the Blueprint → **Apply**.
4. When prompted for `DOC_TOKENS`, paste the JSON token map (next section).
5. Wait for the first build (Rust release build plus LiteParse/model download; typically 5–8
   minutes of the 500 free build minutes per month).

After that, **every push to the connected branch triggers a new build and deploy**
(`autoDeployTrigger: commit` in `render.yaml`). Manual deploys are available from the service page.

## 3. Tokens: one per consuming application

`DOC_TOKENS` is a JSON map of token → tenant. Generate long random tokens and give one to each app:

```sh
openssl rand -hex 32   # repeat per app
# {"<token-app-a>":"app-a","<token-app-b>":"app-b","<token-app-c>":"app-c"}
```

Set it in Render → service → Environment → `DOC_TOKENS` (Blueprint marks it `sync: false`, so Render
prompts instead of storing it in git). All `/v1/*` calls need
`Authorization: Bearer <token>`; `/health/*` stays open for Render health checks.

## 4. Call it from your services

The service is asynchronous because a scan can take longer than a single HTTP request:

```sh
BASE=https://doc-extractor.onrender.com
TOKEN=<token-app-a>

# 1) queue a file (multipart) - returns 202 with an id
JOB=$(curl -s -H "Authorization: Bearer $TOKEN" \
  -F file=@scan.pdf -F 'options={"ocr":true}' \
  "$BASE/v1/extractions" | python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])')

# 2) poll until completed
until curl -s -H "Authorization: Bearer $TOKEN" "$BASE/v1/extractions/$JOB" \
  | grep -q '"state":"completed"'; do sleep 2; done

# 3) fetch and store the result in your app
curl -s -H "Authorization: Bearer $TOKEN" "$BASE/v1/extractions/$JOB/result" > result.json
```

Retry the first request on `502/503/504`: after 15 idle minutes Render spins the service down and
the next request wakes it in about a minute. Once awake, uploads return immediately and polling is
cheap. Treat a `404` on the job id as "the container restarted" and re-upload — the job database is
ephemeral.

Other endpoints: `GET /v1/extractions?q=` for search, `/v1/stats`, `/metrics`, `/docs` (Swagger),
`/` (workbench UI), `/view/{id}`.

## 5. Memory and lifetime

- Render free provides 512 MB, 0.1 CPU, one instance, and 750 instance-hours per month. OCR
  latency depends on the scan and the CPU quota. See the local verification below for measured
  job times.
- The filesystem is ephemeral: SQLite job history, cached results and page rasters disappear on
  every restart or sleep. Persist the results you care about in the calling app (or push them to
  R2/S3 with the `s3` sink).
- The service sleeps after 15 minutes without traffic; the first request can take ~1 minute and may
  return 503 while waking. Retry in clients, or keep it warm with an external ping every 5–10
  minutes (uses most of the 750 hours).
- `RUST_LOG=doc_extractor=info` in the Blueprint keeps per-page timing lines in the Render logs.

## 6. Local verification

```sh
docker build -f Dockerfile.render -t doc-extractor-render .
docker run --rm --memory=512000000 --memory-swap=512000000 --cpus=0.1 \
  -p 10000:10000 \
  -e DOC_TOKENS='{"test":"app-a"}' doc-extractor-render
curl -s localhost:10000/health/ready
```

Then use the same upload/poll/result calls with `http://localhost:10000` and `Bearer test`.

Measured on Linux ARM64 Docker with swap disabled. Each upload was a six-page PDF constructed
from `fixtures/scanned.pdf`. OCRia-nano-1 produced OCR text on every page. Peak memory is the
container's cgroup `memory.peak`, including filesystem cache. Times run from upload to the
last job completing.

| Container budget | Profile | Completed | Peak memory | Time |
|---|---|---:|---:|---:|
| 512 MB / 0.1 CPU | Free | 1 of 1 | 361 MiB | 144.6 s |
| 512 MB / 1 CPU | Free | 4 of 4 | 414 MiB | 49.7 s |
| 2 GB / 1 CPU | 2 GB | 4 of 4 in each of two runs | 814–822 MiB | 31.2–70.4 s |

All runs reported zero cgroup OOM events. Both 2 GB runs reached two active jobs. The same four
uploads on 2 GB / 1 CPU with one active job and the same per-worker memory budget took 33.6 s.
These runs verified concurrency and memory headroom. Batch duration varied across runs.
These measurements were taken before isolating the Render changes for review. Render deployment is unverified.

## 7. Enable two workers on a 2 GB instance

Render's `1c-2g` plan provides one CPU and 2 GB RAM for $25/month. For a Blueprint-managed
upgrade, set `plan: 1c-2g` and add this entry under `envVars` before syncing the Blueprint.
[Render compute plans](https://render.com/docs/compute-plans) · [Render pricing](https://render.com/pricing)

```yaml
- key: DOC_RENDER_CONFIG
  value: /home/extractor/render-2gb.json
```

Run the same profile locally under a 2 GB RAM limit, with swap disabled and one CPU, using
the image built above.

```sh
docker run --rm --memory=2000000000 --memory-swap=2000000000 --cpus=1 \
  -p 10000:10000 -e DOC_TOKENS='{"test":"app-a"}' \
  -e DOC_RENDER_CONFIG=/home/extractor/render-2gb.json doc-extractor-render
```
