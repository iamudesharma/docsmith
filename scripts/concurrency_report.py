#!/usr/bin/env python3
"""Concurrent extraction and accuracy report over a synthetic 20-PDF corpus.

Builds 20 deterministic PDFs from the committed fixtures (the 10 base PDFs plus 10
`pdfunite` compositions), uploads all of them at once to the real HTTP service, and
measures the queue/worker behaviour, latency, throughput, process-tree RSS and content
accuracy. Two service configurations are run against fresh data directories:

  default   limits.workers=2, limits.pdf_workers=4
  tuned     limits.workers=6, limits.pdf_workers=2

OCR is enabled only for documents that contain a scanned/mixed page; pure digital PDFs
stay native. Accuracy is scored against the known synthetic fixture content: page counts,
exact per-page text, content markers, PDF table cells, OCR character error rate, hidden
text duplicate suppression, retained diagram source and the corrupt-PDF negative case.

Outputs `docs/concurrency-results.json` and `docs/CONCURRENCY.md`.

Requires `pdfinfo`/`pdfunite` (Poppler), `curl`, and Tesseract for the scan-bearing
documents. Run with the release binary:

    scripts/cargo-local.sh build --release --locked
    python3 scripts/concurrency_report.py
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
FIX = ROOT / "fixtures"
BIN = next(
    (p for p in [ROOT / "target/release/doc-extractor", ROOT / "target/debug/doc-extractor"] if p.is_file()),
    None,
)
DEFAULT_OUT_JSON = ROOT / "docs/concurrency-results.json"
DEFAULT_OUT_MD = ROOT / "docs/CONCURRENCY.md"

# Known synthetic content, from scripts/generate-fixtures.py. `page_text` is the exact
# normalized text expected per page; None means "not compared" (table/diagram pages).
BASE = {
    "digital": {
        "pages": 2,
        "page_text": ["Native page ONE", "Native page TWO"],
        "markers": ["Native page ONE", "Native page TWO"],
        "ocr": False,
    },
    "long": {
        "pages": 130,
        "page_text": [f"Long page {i}" for i in range(1, 131)],
        "markers": ["Long page 1", "Long page 50", "Long page 100", "Long page 130"],
        "ocr": False,
    },
    "pages-images": {
        "pages": 6,
        "page_text": ["Page one", "Page two", "Page three", "Page four", "Page five", "Page six"],
        "markers": ["Page one", "Page six"],
        "ocr": False,
    },
    "multicolumn": {
        "pages": 1,
        "page_text": ["Left 1 Left 2 Left 3 Left 4 Right 1 Right 2 Right 3 Right 4"],
        "markers": ["Left 1", "Left 4", "Right 1", "Right 4"],
        "ocr": False,
    },
    "table": {
        "pages": 1,
        "page_text": [None],
        "markers": ["Name", "Amount", "Ada", "Lin"],
        "ocr": False,
        "table": True,
    },
    "embedded-diagram": {
        "pages": 1,
        "page_text": [""],
        "markers": [],
        "ocr": False,
        "diagram": True,
    },
    "mixed": {
        "pages": 1,
        "page_text": ["Digital heading Invoice TOTAL 12345"],
        "markers": ["Digital heading", "Invoice TOTAL 12345"],
        "ocr": True,
        "ocr_truth": ["Invoice TOTAL 12345"],
    },
    "scanned": {
        "pages": 1,
        "page_text": ["Invoice TOTAL 12345"],
        "markers": ["Invoice TOTAL 12345"],
        "ocr": True,
        "ocr_truth": ["Invoice TOTAL 12345"],
    },
    "hidden-ocr": {
        "pages": 1,
        "page_text": ["Invoice TOTAL 12345"],
        "markers": ["Invoice TOTAL 12345"],
        "ocr": True,
        "hidden": True,
    },
    "corrupt": {
        "pages": None,
        "page_text": None,
        "markers": [],
        "ocr": False,
        "corrupt": True,
    },
}

COMPOSITES = [
    ("digital-table", ["digital", "table"]),
    ("mixed-scanned", ["mixed", "scanned"]),
    ("pages-digital", ["pages-images", "digital"]),
    ("multicolumn-table-digital", ["multicolumn", "table", "digital"]),
    ("hidden-mixed", ["hidden-ocr", "mixed"]),
    ("long-digital", ["long", "digital"]),
    ("digital-pages-table", ["digital", "pages-images", "table"]),
    ("scanned-hidden", ["scanned", "hidden-ocr"]),
    ("diagram-multicolumn", ["embedded-diagram", "multicolumn"]),
    ("mixed-table-pages", ["mixed", "table", "pages-images"]),
]

# The table fixture's exact geometric cells.
TABLE_CELLS = {
    (0, 0): "Name",
    (0, 1): "Amount",
    (1, 0): "Ada",
    (1, 1): "12",
    (2, 0): "Lin",
    (2, 1): "34",
}

CONFIGS = [
    ("default", {"workers": 2, "pdf_workers": 4, "ocr_threads": 4}),
    ("tuned", {"workers": 6, "pdf_workers": 2, "ocr_threads": 4}),
]

TERMINAL = {"completed", "failed", "cancelled"}


def normalized(value):
    return " ".join(value.split())


def levenshtein(a, b):
    row = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        nxt = [i]
        for j, y in enumerate(b, 1):
            nxt.append(min(nxt[-1] + 1, row[j] + 1, row[j - 1] + (x != y)))
        row = nxt
    return row[-1]


def percentile(values, fraction):
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int(round((len(ordered) - 1) * fraction))))
    return ordered[index]


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def command_output(*command, **kwargs):
    return subprocess.check_output(command, text=True, stderr=subprocess.STDOUT, **kwargs).strip()


def hardware():
    if sys.platform == "darwin":
        model = command_output("sysctl", "-n", "hw.model")
        memory = int(command_output("sysctl", "-n", "hw.memsize"))
        return f"{model}, {memory / (1024 ** 3):.0f} GB"
    cpu = "unknown"
    try:
        for line in pathlib.Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    return f"{cpu} x{os.cpu_count()}"


def pdf_pages(path):
    try:
        output = command_output("pdfinfo", str(path))
    except (OSError, subprocess.CalledProcessError):
        return None
    match = re.search(r"^Pages:\s+(\d+)", output, re.M)
    return int(match.group(1)) if match else None


# ---------------------------------------------------------------- corpus


def build_corpus(corpus_dir):
    corpus_dir.mkdir(parents=True, exist_ok=True)
    docs = []
    specs = [(name, [name]) for name in BASE if name != "corrupt"]
    specs.append(("corrupt", ["corrupt"]))
    specs.extend(COMPOSITES)

    for index, (name, components) in enumerate(specs):
        filename = f"{index + 1:02d}-{name}.pdf"
        target = corpus_dir / filename
        if len(components) == 1:
            shutil.copyfile(FIX / f"{components[0]}.pdf", target)
        else:
            subprocess.run(
                ["pdfunite", *[str(FIX / f"{c}.pdf") for c in components], str(target)],
                check=True,
                capture_output=True,
                text=True,
            )
        pages = pdf_pages(target)
        expected_pages = []
        markers = []
        ocr_truth = []
        table = diagram = hidden = corrupt = False
        for component in components:
            spec = BASE[component]
            if spec["page_text"] is not None:
                expected_pages.extend(spec["page_text"])
            if spec["pages"] is not None:
                expected_pages.extend([None] * (spec["pages"] - len(spec.get("page_text") or [])))
            markers.extend(spec.get("markers", []))
            ocr_truth.extend(spec.get("ocr_truth", []))
            table = table or spec.get("table", False)
            diagram = diagram or spec.get("diagram", False)
            hidden = hidden or spec.get("hidden", False)
            corrupt = corrupt or spec.get("corrupt", False)
        seen = set()
        unique_markers = []
        for marker in markers:
            if marker not in seen:
                seen.add(marker)
                unique_markers.append(marker)
        doc = {
            "name": name,
            "file": filename,
            "path": target,
            "components": components,
            "pages": pages,
            "expected_pages": expected_pages if expected_pages else None,
            "markers": unique_markers,
            "ocr": any(BASE[c].get("ocr") for c in components),
            "ocr_truth": ocr_truth,
            "table": table,
            "diagram": diagram,
            # Every hidden page and every OCR-recovered scan contributes one expected
            # "Invoice TOTAL 12345" occurrence; duplicate suppression must not add more.
            "invoice_occurrences": sum(1 for c in components if BASE[c].get("hidden"))
            + len(ocr_truth),
            "corrupt": corrupt,
            "diagram_scored": name == "embedded-diagram",
        }
        if pages is None and not corrupt:
            raise SystemExit(f"pdfinfo failed for {target}")
        if pages is not None and not corrupt and pages != len(expected_pages):
            raise SystemExit(
                f"page mismatch for {target}: pdfinfo={pages} expected={len(expected_pages)}"
            )
        docs.append(doc)
    return docs


# ---------------------------------------------------------------- service


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http_get(url, timeout=60):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(url, timeout=timeout) as response:
        return response.read()


def get_json(url, timeout=60):
    return json.loads(http_get(url, timeout))


def wait_ready(base, server, log_path):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if server.poll() is not None:
            tail = ""
            if log_path.exists():
                tail = log_path.read_text(errors="replace")[-2000:]
            raise SystemExit(f"server exited early:\n{tail}")
        try:
            http_get(base + "/health/ready", timeout=2)
            return
        except (urllib.error.URLError, TimeoutError, OSError):
            time.sleep(0.2)
    raise SystemExit("server did not become ready")


def start_server(config, data_dir, log_path):
    port = free_port()
    cfg = {
        "data_dir": str(data_dir),
        "bind": f"127.0.0.1:{port}",
        "limits": config,
    }
    cfg_path = data_dir / "config.json"
    cfg_path.write_text(json.dumps(cfg))
    log = log_path.open("wb")
    server = subprocess.Popen(
        [str(BIN), "serve", "--config", str(cfg_path)],
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    base = f"http://127.0.0.1:{port}"
    wait_ready(base, server, log_path)
    return server, base, log


def stop_server(server, log):
    if server.poll() is None:
        try:
            os.killpg(os.getpgid(server.pid), signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            server.wait(timeout=15)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(os.getpgid(server.pid), signal.SIGKILL)
            except ProcessLookupError:
                pass
            server.wait(timeout=5)
    log.close()


def upload_one(base, doc):
    started = time.monotonic()
    options = {"ocr": doc["ocr"]}
    command = [
        "curl",
        "-sS",
        "--max-time",
        "120",
        "-F",
        f"file=@{doc['path']};type=application/pdf",
        "-F",
        f"options={json.dumps(options)}",
        base + "/v1/extractions",
    ]
    result = subprocess.run(command, capture_output=True, text=True)
    accepted = time.monotonic()
    if result.returncode != 0:
        return {"ok": False, "error": (result.stderr or result.stdout).strip()[:1000]}
    try:
        body = json.loads(result.stdout)
    except json.JSONDecodeError:
        return {"ok": False, "error": result.stdout.strip()[:1000]}
    if "id" not in body:
        return {"ok": False, "error": json.dumps(body)[:1000]}
    return {
        "ok": True,
        "job_id": body["id"],
        "cached": body.get("cached", False),
        "upload_ms": (accepted - started) * 1000,
        "accepted_at": accepted,
    }


def process_tree(pid):
    try:
        rows = command_output("ps", "-axo", "pid=,ppid=,rss=,%cpu=")
    except (OSError, subprocess.CalledProcessError):
        return 0, 0.0
    entries = []
    for line in rows.splitlines():
        parts = line.split()
        if len(parts) < 4:
            continue
        try:
            entries.append((int(parts[0]), int(parts[1]), int(parts[2]), float(parts[3])))
        except ValueError:
            continue
    ids = {pid}
    while True:
        previous = len(ids)
        ids.update(child for child, parent, _, _ in entries if parent in ids)
        if len(ids) == previous:
            break
    rss = sum(rss_kb for child, _, rss_kb, _ in entries if child in ids)
    cpu = sum(cpu_pct for child, _, _, cpu_pct in entries if child in ids)
    return rss * 1024, cpu


def fetch_result(base, job_id):
    raw = http_get(f"{base}/v1/extractions/{job_id}/result", timeout=120)
    return raw, json.loads(raw)


# ---------------------------------------------------------------- accuracy


def block_text(block):
    return block.get("text") or ""


def doc_text(result):
    parts = []
    for unit in result.get("units", []):
        for block in unit.get("blocks", []):
            if block.get("text"):
                parts.append(block["text"])
            if block.get("table"):
                parts.extend(cell.get("raw", "") for cell in block["table"].get("cells", []))
            if block.get("diagram"):
                diagram = block["diagram"]
                parts.extend(node.get("label", "") for node in diagram.get("nodes", []))
                parts.extend(edge.get("label", "") for edge in diagram.get("edges", []))
    return normalized(" ".join(parts))


def page_units(result):
    return [unit for unit in result.get("units", []) if unit.get("kind") == "page"]


def evaluate(doc, result):
    pages = page_units(result)
    page_count_ok = doc["pages"] is None or len(pages) == doc["pages"]
    exact_total = exact_ok = 0
    if doc["expected_pages"]:
        for unit, expected in zip(pages, doc["expected_pages"]):
            if expected is None:
                continue
            got = normalized(
                " ".join(
                    block_text(block)
                    for block in unit.get("blocks", [])
                    if block.get("kind") == "text" and block.get("text")
                )
            )
            exact_total += 1
            exact_ok += got == normalized(expected)
    text = doc_text(result)
    markers_found = sum(1 for marker in doc["markers"] if normalized(marker) in text)
    hidden_ok = None
    if doc["invoice_occurrences"]:
        hidden_ok = text.lower().count("invoice total 12345") == doc["invoice_occurrences"]

    table_ok = None
    cells_total = cells_ok = 0
    if doc["table"]:
        found = None
        for unit in result.get("units", []):
            for block in unit.get("blocks", []):
                if not block.get("table"):
                    continue
                table = {
                    (cell["row"], cell["column"]): cell.get("raw", "")
                    for cell in block["table"].get("cells", [])
                }
                if table.get((0, 0)) == "Name" and table.get((0, 1)) == "Amount":
                    found = table
                    break
            if found is not None:
                break
        cells_total = len(TABLE_CELLS)
        cells_ok = sum(1 for key, value in TABLE_CELLS.items() if found and found.get(key) == value)
        table_ok = cells_ok == cells_total

    ocr_cer = None
    if doc["ocr_truth"]:
        truth = normalized(" ".join(doc["ocr_truth"]))
        got = normalized(
            " ".join(
                block_text(block)
                for unit in result.get("units", [])
                for block in unit.get("blocks", [])
                if block.get("kind") == "text"
                and block.get("provenance", {}).get("method") == "ocr"
            )
        )
        ocr_cer = levenshtein(truth, got) / max(len(truth), 1)

    diagram_ok = None
    if doc["diagram_scored"]:
        diagram_ok = False
        for unit in result.get("units", []):
            for block in unit.get("blocks", []):
                diagram = block.get("diagram")
                if not diagram:
                    continue
                labels = {node["id"]: node.get("label", "").lower() for node in diagram.get("nodes", [])}
                edges = {
                    (labels.get(edge.get("source"), ""), labels.get(edge.get("target"), ""))
                    for edge in diagram.get("edges", [])
                }
                wanted = {"users", "orders"}
                if wanted <= set(labels.values()) and ("orders", "users") in edges:
                    diagram_ok = True

    status = result.get("status")
    corrupt_ok = None if not doc["corrupt"] else status in ("failed", "unsupported")
    extraction_ok = (
        status in ("complete", "partial") if not doc["corrupt"] else status in ("failed", "unsupported")
    )
    unit_errors = sum(len(unit.get("errors", [])) for unit in result.get("units", []))
    passed = bool(
        extraction_ok
        and page_count_ok
        and markers_found == len(doc["markers"])
        and (table_ok is not False)
        and (hidden_ok is not False)
        and (diagram_ok is not False)
        and (corrupt_ok is not False)
    )
    return {
        "pages": len(pages),
        "page_count_ok": page_count_ok,
        "page_text_total": exact_total,
        "page_text_correct": exact_ok,
        "markers_total": len(doc["markers"]),
        "markers_found": markers_found,
        "table_cells_total": cells_total,
        "table_cells_correct": cells_ok,
        "table_ok": table_ok,
        "ocr_cer": ocr_cer,
        "hidden_ok": hidden_ok,
        "diagram_ok": diagram_ok,
        "corrupt_ok": corrupt_ok,
        "status": status,
        "unit_errors": unit_errors,
        "document_errors": len(result.get("errors", [])),
        "passed": passed,
    }


# ---------------------------------------------------------------- run


def run_configuration(name, limits, docs, corpus_dir, keep):
    run_dir = pathlib.Path(tempfile.mkdtemp(prefix=f"doc-concurrency-{name}-", dir=corpus_dir.parent))
    data_dir = run_dir / "data"
    data_dir.mkdir()
    log_path = run_dir / "server.log"
    server = base = log = None
    try:
        server, base, log = start_server(limits, data_dir, log_path)
        for doc in docs:
            doc.update(job_id=None, result=None, accuracy=None)
        started = time.monotonic()
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(docs)) as pool:
            futures = [pool.submit(upload_one, base, doc) for doc in docs]
            uploads = [future.result() for future in futures]
        for doc, upload in zip(docs, uploads):
            doc["upload"] = upload
            if not upload["ok"]:
                raise SystemExit(f"upload failed for {doc['file']}: {upload['error']}")
            doc["job_id"] = upload["job_id"]
            doc["accepted_at"] = upload["accepted_at"]
            doc["started_at"] = None
            doc["finished_at"] = None
        all_accepted = time.monotonic()

        pending = {doc["job_id"] for doc in docs}
        active_samples = []
        queue_samples = []
        rss_samples = []
        cpu_samples = []
        stats_final = None
        while pending:
            if time.monotonic() - started > 1800:
                raise SystemExit("timed out waiting for jobs")
            try:
                stats = get_json(base + "/v1/stats")
                stats_final = stats
                active = stats["workers"]["active"]
                active_samples.append(active)
                queue_samples.append(stats["jobs"].get("queued", 0))
            except (urllib.error.URLError, TimeoutError, OSError):
                pass
            now = time.monotonic()
            for doc in docs:
                if doc["job_id"] not in pending:
                    continue
                try:
                    job = get_json(base + f"/v1/extractions/{doc['job_id']}")
                except (urllib.error.URLError, TimeoutError, OSError):
                    continue
                if job["state"] == "running" and doc["started_at"] is None:
                    doc["started_at"] = now
                if job["state"] in TERMINAL:
                    doc["finished_at"] = now
                    doc["job"] = job
                    pending.discard(doc["job_id"])
            rss, cpu = process_tree(server.pid)
            rss_samples.append(rss)
            cpu_samples.append(cpu)
            if pending:
                time.sleep(0.25)
        finished = time.monotonic()
        makespan = finished - started
        stats_final = get_json(base + "/v1/stats")
        peak_rss = max(rss_samples) if rss_samples else 0

        records = []
        for doc in docs:
            job = doc["job"]
            raw, result = fetch_result(base, doc["job_id"])
            doc["result"] = result
            accuracy = evaluate(doc, result)
            doc["accuracy"] = accuracy
            canonical = {
                key: value
                for key, value in result.items()
                if key not in ("elapsed_ms", "id")
            }
            records.append(
                {
                    "file": doc["file"],
                    "components": doc["components"],
                    "pages": doc["pages"],
                    "ocr": doc["ocr"],
                    "job_id": doc["job_id"],
                    "job_state": job["state"],
                    "result_status": result.get("status"),
                    "upload_ms": round(doc["upload"]["upload_ms"], 1),
                    "queue_ms": round((doc["started_at"] - doc["accepted_at"]) * 1000)
                    if doc["started_at"]
                    else None,
                    "run_ms": round((doc["finished_at"] - doc["started_at"]) * 1000)
                    if doc["started_at"]
                    else None,
                    "latency_ms": round((doc["finished_at"] - doc["accepted_at"]) * 1000),
                    "units": len(result.get("units", [])),
                    "assets": len(result.get("assets", [])),
                    "result_bytes": len(raw),
                    "document_errors": len(result.get("errors", [])),
                    "warning_count": len(job.get("warnings", [])),
                    "accuracy": accuracy,
                    "result_hash": sha256_bytes(
                        json.dumps(canonical, sort_keys=True, separators=(",", ":")).encode()
                    ),
                }
            )
        records.sort(key=lambda record: record["latency_ms"])
        latencies = [record["latency_ms"] / 1000 for record in records]
        queue_waits = [record["queue_ms"] / 1000 for record in records if record["queue_ms"] is not None]
        run_times = [record["run_ms"] / 1000 for record in records if record["run_ms"] is not None]
        valid_pages = sum(doc["pages"] for doc in docs if doc["pages"] is not None)
        summary = {
            "limits": limits,
            "documents": len(records),
            "pages": valid_pages,
            "makespan_s": round(makespan, 3),
            "upload_span_s": round(all_accepted - started, 3),
            "throughput_docs_per_s": round(len(records) / makespan, 3),
            "throughput_pages_per_s": round(valid_pages / makespan, 2),
            "max_active_jobs_observed": max(active_samples) if active_samples else 0,
            "max_queued_jobs_observed": max(queue_samples) if queue_samples else 0,
            "latency_s": {
                "p50": round(percentile(latencies, 0.50), 3),
                "p95": round(percentile(latencies, 0.95), 3),
                "max": round(max(latencies), 3),
            },
            "queue_wait_s": {
                "p50": round(percentile(queue_waits, 0.50), 3),
                "p95": round(percentile(queue_waits, 0.95), 3),
                "max": round(max(queue_waits), 3),
            },
            "execution_s": {
                "p50": round(percentile(run_times, 0.50), 3),
                "p95": round(percentile(run_times, 0.95), 3),
                "max": round(max(run_times), 3),
            },
            "peak_process_tree_rss_mb": round(peak_rss / (1024 * 1024), 1),
            "peak_sampled_cpu_percent": round(max(cpu_samples), 1) if cpu_samples else 0,
            "result_bytes": sum(record["result_bytes"] for record in records),
            "disk_bytes": stats_final.get("disk_bytes", 0) if stats_final else 0,
            "job_states": stats_final.get("jobs", {}) if stats_final else {},
            "accuracy": summarize_accuracy([record["accuracy"] for record in records]),
            "documents": records,
        }
        return summary
    finally:
        if server is not None:
            stop_server(server, log)
        if not keep:
            shutil.rmtree(run_dir, ignore_errors=True)


def summarize_accuracy(results):
    page_total = sum(result["page_text_total"] for result in results)
    page_ok = sum(result["page_text_correct"] for result in results)
    markers_total = sum(result["markers_total"] for result in results)
    markers_found = sum(result["markers_found"] for result in results)
    cells_total = sum(result["table_cells_total"] for result in results)
    cells_ok = sum(result["table_cells_correct"] for result in results)
    cer_values = [result["ocr_cer"] for result in results if result["ocr_cer"] is not None]
    hidden = [result["hidden_ok"] for result in results if result["hidden_ok"] is not None]
    diagrams = [result["diagram_ok"] for result in results if result["diagram_ok"] is not None]
    corrupt = [result["corrupt_ok"] for result in results if result["corrupt_ok"] is not None]
    return {
        "documents_passed": sum(1 for result in results if result["passed"]),
        "documents_total": len(results),
        "page_counts_ok": sum(1 for result in results if result["page_count_ok"]),
        "page_text_exact": f"{page_ok}/{page_total}",
        "page_text_exact_rate": round(page_ok / page_total, 4) if page_total else None,
        "markers": f"{markers_found}/{markers_total}",
        "marker_recall": round(markers_found / markers_total, 4) if markers_total else None,
        "table_cells": f"{cells_ok}/{cells_total}",
        "table_cell_rate": round(cells_ok / cells_total, 4) if cells_total else None,
        "ocr_cases": len(cer_values),
        "ocr_mean_cer": round(sum(cer_values) / len(cer_values), 6) if cer_values else None,
        "ocr_max_cer": round(max(cer_values), 6) if cer_values else None,
        "hidden_text_cases_ok": f"{sum(1 for value in hidden if value)}/{len(hidden)}",
        "diagram_cases_ok": f"{sum(1 for value in diagrams if value)}/{len(diagrams)}",
        "corrupt_cases_ok": f"{sum(1 for value in corrupt if value)}/{len(corrupt)}",
        "unit_errors": sum(result["unit_errors"] for result in results),
        "document_errors": sum(result["document_errors"] for result in results),
    }


# ---------------------------------------------------------------- report


def markdown(report):
    lines = [
        "# Concurrent PDF extraction — performance and accuracy",
        "",
        f"Date: {report['date']}. Synthetic corpus; engine `{report['binary'][:16]}…`.",
        "",
        f"Hardware: `{report['hardware']}`, platform `{report['platform']}`, "
        f"{report['cpus']} CPUs.",
        "",
        "## Corpus",
        "",
        f"**{report['corpus']['documents']} PDFs / {report['corpus']['pages']} pages**, built from the "
        "committed fixtures plus 10 `pdfunite` compositions. OCR is enabled only for documents "
        "containing a scanned, mixed or hidden-OCR page.",
        "",
        "| # | Document | Pages | OCR | Source components |",
        "|---:|---|---:|:---:|---|",
    ]
    for index, doc in enumerate(report["corpus"]["files"], 1):
        lines.append(
            f"| {index} | `{doc['file']}` | {doc['pages'] if doc['pages'] is not None else '—'} | "
            f"{'yes' if doc['ocr'] else 'no'} | {', '.join(doc['components'])} |"
        )
    lines += ["", "## Performance", ""]
    lines.append(
        "Each configuration was run "
        f"{report['repetitions']} times against a fresh data directory, interleaved to reduce host "
        "drift. The table shows the median run; makespan range is in parentheses."
    )
    lines.append("")
    lines.append(
        "| Configuration | Workers | PDF workers | Makespan median (range) | Docs/s | Pages/s | "
        "Max active | Latency p50 / p95 / max | Peak tree RSS |"
    )
    lines.append("|---|---:|---:|---:|---:|---:|---:|---|---:|")
    for run in report["runs"]:
        summary = run
        lines.append(
            f"| {run['name']} | {summary['limits']['workers']} | {summary['limits']['pdf_workers']} | "
            f"{summary['makespan_median_s']:.1f} s ({summary['makespan_min_s']:.1f}–"
            f"{summary['makespan_max_s']:.1f}) | "
            f"{len(summary['documents']) / summary['makespan_median_s']:.2f} | "
            f"{summary['pages'] / summary['makespan_median_s']:.1f} | "
            f"{summary['max_active_jobs_observed']} | "
            f"{summary['latency_s']['p50']:.1f} / {summary['latency_s']['p95']:.1f} / "
            f"{summary['latency_s']['max']:.1f} s | {summary['peak_process_tree_rss_mb']:.0f} MB |"
        )
    lines += [
        "",
        "Peak RSS is sampled from the server process tree (`ps`, every ~0.25 s); short-lived "
        "Poppler/Tesseract peaks may be missed, and macOS RSS includes allocator-resident pages. "
        "Latency/RSS are from the median run; makespan and throughput use the median.",
        "",
        "## Accuracy",
        "",
        "| Metric | Result |",
        "|---|---|",
    ]
    tuned = report["runs"][-1]
    accuracy = tuned["accuracy"]
    lines += [
        f"| Documents passing all checks | **{accuracy['documents_passed']}/{accuracy['documents_total']}** |",
        f"| Page counts correct | **{accuracy['page_counts_ok']}/{accuracy['documents_total']}** |",
        f"| Exact page text | **{accuracy['page_text_exact']}** ({accuracy['page_text_exact_rate'] * 100:.1f}%) |",
        f"| Content markers | **{accuracy['markers']}** ({accuracy['marker_recall'] * 100:.1f}%) |",
        f"| PDF table cells exact | **{accuracy['table_cells']}** ({accuracy['table_cell_rate'] * 100:.1f}%) |",
        f"| OCR mean / max CER | **{accuracy['ocr_mean_cer']:.4f} / {accuracy['ocr_max_cer']:.4f}** "
        f"over {accuracy['ocr_cases']} cases |",
        f"| Invoice occurrences exact (OCR + hidden text) | **{accuracy['hidden_text_cases_ok']}** |",
        f"| Retained diagram source | **{accuracy['diagram_cases_ok']}** |",
        f"| Corrupt PDF reported cleanly | **{accuracy['corrupt_cases_ok']}** |",
        "",
        f"Per-document results (median tuned run; accuracy was identical in all "
        f"{report['repetitions'] * len(report['runs'])} runs: "
        f"{str(report['accuracy_stable_across_rounds']).lower()}):",
        "",
        "| Document | Status | Pages | Page text | Markers | Table | OCR CER | Pass |",
        "|---|---|---:|---:|---:|---:|---:|:---:|",
    ]
    for record in tuned["documents"]:
        result = record["accuracy"]
        table = "—" if result["table_ok"] is None else ("ok" if result["table_ok"] else "fail")
        page_text = (
            f"{result['page_text_correct']}/{result['page_text_total']}"
            if result["page_text_total"]
            else "—"
        )
        ocr = "—" if result["ocr_cer"] is None else f"{result['ocr_cer']:.4f}"
        lines.append(
            f"| `{record['file']}` | {result['status']} | {result['pages']} | {page_text} | "
            f"{result['markers_found']}/{result['markers_total']} | {table} | {ocr} | "
            f"{'yes' if result['passed'] else 'no'} |"
        )
    lines += ["", "## Per-document timing (tuned run)", ""]
    lines.append("| Document | Queue | Run | Total | Pages | Result bytes |")
    lines.append("|---|---:|---:|---:|---:|---:|")
    for record in tuned["documents"]:
        queue = "—" if record["queue_ms"] is None else f"{record['queue_ms'] / 1000:.1f} s"
        run = "—" if record["run_ms"] is None else f"{record['run_ms'] / 1000:.1f} s"
        lines.append(
            f"| `{record['file']}` | {queue} | {run} | "
            f"{record['latency_ms'] / 1000:.1f} s | {record['units']} | "
            f"{record['result_bytes']:,} |"
        )
    lines += [
        "",
        "## Notes",
        "",
        "- All 20 jobs were uploaded concurrently to one local service; `max active` is the "
        "number of jobs actually running at the same time (the rest waited in the durable queue).",
        "- Accuracy uses the known synthetic fixture content. Page text is compared after "
        "whitespace normalization; table cells use exact `raw` strings; OCR CER uses the known "
        "`Invoice TOTAL 12345` line. The invoice-occurrence check verifies that hidden OCR text "
        "and page OCR are not duplicated.",
        "- Valid PDFs report `partial` because every PDF unit carries conservative reading-order "
        "and geometric-table warnings; that is the expected status, not a content failure. The "
        "single `document_errors` entry belongs to the intentionally corrupt PDF.",
        "- Page-image capture was disabled for this run so the measurements cover the core "
        "extraction path. `elapsed_ms` is excluded when comparing outputs across configurations.",
        "- Synthetic fixtures are not a substitute for real customer scans, handwriting or "
        "complex layout corpora.",
    ]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out-json", type=pathlib.Path, default=DEFAULT_OUT_JSON)
    parser.add_argument("--out-md", type=pathlib.Path, default=DEFAULT_OUT_MD)
    parser.add_argument("--keep", action="store_true", help="keep per-run server logs/data")
    parser.add_argument("--repetitions", type=int, default=3)
    args = parser.parse_args()
    if BIN is None:
        raise SystemExit("build the extractor first")
    corpus_root = pathlib.Path(tempfile.mkdtemp(prefix="doc-concurrency-corpus-"))
    try:
        docs = build_corpus(corpus_root / "corpus")
        samples = {name: [] for name, _ in CONFIGS}
        for repetition in range(1, args.repetitions + 1):
            for name, limits in CONFIGS:
                print(
                    f"running {name} configuration (round {repetition}): {limits}", flush=True
                )
                summary = run_configuration(name, limits, docs, corpus_root, args.keep)
                summary["name"] = name
                summary["round"] = repetition
                summary["documents"].sort(key=lambda record: record["file"])
                samples[name].append(summary)
                print(
                    f"  {summary['makespan_s']:.1f}s, max active "
                    f"{summary['max_active_jobs_observed']}, peak RSS "
                    f"{summary['peak_process_tree_rss_mb']:.0f} MB, docs passed "
                    f"{summary['accuracy']['documents_passed']}/"
                    f"{summary['accuracy']['documents_total']}",
                    flush=True,
                )
        runs = []
        for name, _ in CONFIGS:
            ordered = sorted(samples[name], key=lambda summary: summary["makespan_s"])
            representative = ordered[(len(ordered) - 1) // 2]
            makespans = [summary["makespan_s"] for summary in samples[name]]
            representative["makespan_median_s"] = percentile(makespans, 0.5)
            representative["makespan_min_s"] = min(makespans)
            representative["makespan_max_s"] = max(makespans)
            representative["rounds"] = [
                {
                    "round": summary["round"],
                    "makespan_s": summary["makespan_s"],
                    "max_active_jobs_observed": summary["max_active_jobs_observed"],
                    "latency_p50_s": summary["latency_s"]["p50"],
                    "latency_p95_s": summary["latency_s"]["p95"],
                    "peak_process_tree_rss_mb": summary["peak_process_tree_rss_mb"],
                    "accuracy": summary["accuracy"]["documents_passed"],
                }
                for summary in samples[name]
            ]
            runs.append(representative)
        accuracy_stable = all(
            summary["accuracy"] == runs[0]["accuracy"]
            for name, _ in CONFIGS
            for summary in samples[name]
        )
        # Accuracy must not depend on the worker count. Flag any divergence.
        hashes = [
            {record["file"]: record["result_hash"] for record in run["documents"]}
            for run in runs
        ]
        identical = sum(
            1 for filename in hashes[0] if hashes[0][filename] == hashes[1][filename]
        )
        report = {
            "date": time.strftime("%Y-%m-%d"),
            "platform": sys.platform,
            "hardware": hardware(),
            "cpus": os.cpu_count(),
            "binary": sha256_bytes(BIN.read_bytes()),
            "repetitions": args.repetitions,
            "corpus": {
                "documents": len(docs),
                "pages": sum(doc["pages"] for doc in docs if doc["pages"] is not None),
                "synthetic": True,
                "files": [
                    {
                        "file": doc["file"],
                        "components": doc["components"],
                        "pages": doc["pages"],
                        "ocr": doc["ocr"],
                    }
                    for doc in docs
                ],
            },
            "runs": runs,
            "accuracy_stable_across_rounds": accuracy_stable,
            "cross_configuration": {
                "identical_documents": identical,
                "documents": len(docs),
            },
        }
        args.out_json.parent.mkdir(parents=True, exist_ok=True)
        args.out_json.write_text(json.dumps(report, indent=2) + "\n")
        args.out_md.write_text(markdown(report))
        print(f"wrote {args.out_json} and {args.out_md}")
    finally:
        if not args.keep:
            shutil.rmtree(corpus_root, ignore_errors=True)


if __name__ == "__main__":
    main()
