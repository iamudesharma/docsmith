#!/usr/bin/env python3
"""Benchmark large local inputs against a real service instance and record evidence.

Generates synthetic digital / scanned / mixed / image-heavy PDFs and a multi-million-row CSV,
runs each through `doc-extractor serve` (one worker), samples process-tree RSS, reads the
per-run stage timings (`timings.json`), records result/job disk usage, and exercises crash
recovery (`SIGKILL` mid-run, re-submit) and cancellation (no surviving descendants or temp
files). Writes `docs/perf-large-inputs.json`.

Usage: python3 scripts/benchmark-large.py [--work-dir DIR] [--quick]
Not run in CI: it needs Pillow, Poppler and Tesseract and several minutes.
"""
import io, json, os, random, shutil, signal, socket, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/release/doc-extractor"
WORK = Path(sys.argv[sys.argv.index("--work-dir") + 1]) if "--work-dir" in sys.argv else Path("/tmp/docsmith-bench-large")
QUICK = "--quick" in sys.argv

def jpeg(text, size, seed):
    from PIL import Image, ImageDraw
    img = Image.new("RGB", size, "white")
    d = ImageDraw.Draw(img)
    d.text((20, 40), text, fill="black")
    d.text((20, 90), f"scan line {seed} alpha beta gamma delta", fill="black")
    d.text((20, 140), "invoice total 120.50 units", fill="black")
    out = io.BytesIO()
    img.save(out, format="JPEG", quality=70)
    return out.getvalue()

def build_pdf(path, pages, layout):
    """layout(page_number) -> (text_lines, [(jpeg_bytes, scale_w, scale_h), ...])"""
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    image_ids = {}
    def image_object(key, data, w, h):
        if key in image_ids:
            return image_ids[key]
        pid = len(objects) + 1
        objects.append(
            b"<< /Type /XObject /Subtype /Image /Width "
            + str(w).encode()
            + b" /Height "
            + str(h).encode()
            + b" /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length "
            + str(len(data)).encode()
            + b" >>\nstream\n"
            + data
            + b"\nendstream"
        )
        image_ids[key] = pid
        return pid
    kids = []
    for i in range(1, pages + 1):
        lines, images = layout(i)
        content = b""
        for r, line in enumerate(lines):
            content += f"BT /F1 12 Tf 40 {760 - 18 * r} Td ({line}) Tj ET\n".encode()
        resources = b""
        for j, (data, w, h) in enumerate(images):
            pid = image_object((i, j), data, w, h)
            y = 120 + 160 * j
            content += f"q 500 0 0 {min(150, h)} 40 {y} cm /Im{pid} Do Q\n".encode()
            resources += f"/Im{pid} {pid} 0 R ".encode()
        pid = len(objects) + 1
        cid = pid + 1
        objects.append(
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> /XObject << {resources.decode()} >> >> /Contents {cid} 0 R >>".encode()
        )
        objects.append(b"<< /Length " + str(len(content)).encode() + b" >>\nstream\n" + content + b"endstream")
        kids.append(pid)
    objects[1] = (
        f"<< /Type /Pages /Count {len(kids)} /Kids [".encode()
        + b" ".join(f"{i} 0 R".encode() for i in kids)
        + b"] >>"
    )
    out = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n".encode() + obj + b"\nendobj\n"
    start = len(out)
    out += f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode()
    for off in offsets[1:]:
        out += f"{off:010d} 00000 n \n".encode()
    out += f"trailer << /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n".encode()
    path.write_bytes(out)
    return len(out)

def gen_inputs(work):
    work.mkdir(parents=True, exist_ok=True)
    inputs = {}
    digital = QUICK and 400 or 2000
    def make_digital(prefix):
        def layout(i):
            lines = [f"{prefix} page {i} heading"] + [f"Body line {r} of page {i} with several words" for r in range(1, 13)]
            return lines, []
        return layout
    inputs["digital.pdf"] = {"pages": digital, "shape": "text page, shared figure every 25th", "bytes": build_pdf(work / "digital.pdf", digital, make_digital("Digital"))}
    inputs["recovery.pdf"] = {"pages": digital, "shape": "digital variant used for the SIGKILL recovery run", "bytes": build_pdf(work / "recovery.pdf", digital, make_digital("Recovery"))}

    scans = QUICK and 24 or 80
    def scan_layout(i):
        return [], [(jpeg(f"Scanned page {i} report text", (900, 1200), i), 900, 1200)]
    inputs["scanned.pdf"] = {"pages": scans, "shape": "image-only page per scan", "bytes": build_pdf(work / "scanned.pdf", scans, scan_layout)}

    def cancel_layout(i):
        return [], [(jpeg(f"Cancellation page {i} text", (900, 1200), 500 + i), 900, 1200)]
    inputs["cancel.pdf"] = {"pages": scans, "shape": "scan variant used for the cancellation run", "bytes": build_pdf(work / "cancel.pdf", scans, cancel_layout)}

    text_pages = QUICK and 20 or 60
    scan_pages = QUICK and 12 or 40
    def mixed_layout(i):
        if i <= text_pages:
            return [f"Chapter page {i}"] + [f"Prose line {r} on page {i}" for r in range(1, 10)], []
        return [], [(jpeg(f"Mixed scan page {i}", (900, 1200), i), 900, 1200)]
    inputs["mixed.pdf"] = {"pages": text_pages + scan_pages, "shape": f"{text_pages} digital pages + {scan_pages} scan pages", "bytes": build_pdf(work / "mixed.pdf", text_pages + scan_pages, mixed_layout)}

    heavy = QUICK and 12 or 30
    def heavy_layout(i):
        return [], [(jpeg(f"Image heavy page {i} panel {j}", (450, 300), i * 10 + j), 450, 300) for j in range(3)]
    inputs["image-heavy.pdf"] = {"pages": heavy, "shape": "3 scanned panels per page", "bytes": build_pdf(work / "image-heavy.pdf", heavy, heavy_layout)}

    rows = QUICK and 500_000 or 2_000_000
    csv_path = work / "rows.csv"
    rand = random.Random(7)
    with open(csv_path, "w") as f:
        f.write("id,name,amount,notes\n")
        for i in range(rows):
            f.write(f"{i},user{i % 1000},{rand.random() * 1000:.3f},\"note {i} alpha\"\n")
    inputs["rows.csv"] = {"rows": rows, "shape": f"{rows} rows x 4 columns", "bytes": csv_path.stat().st_size}
    return inputs

def host_description():
    """Portable host label; never fatal (a cosmetic probe must not fail a run)."""
    if sys.platform == "darwin":
        try:
            return subprocess.check_output(
                ["sysctl", "-n", "machdep.cpu.brand_string"], text=True
            ).strip()
        except Exception:
            return "macOS"
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.lower().startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return sys.platform

def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

def start_service(work, limits):
    port = free_port()
    cfg = {"data_dir": str(work / "data"), "bind": f"127.0.0.1:{port}", "limits": limits}
    (work / "config.json").write_text(json.dumps(cfg))
    log = open(work / "server.log", "a")
    proc = subprocess.Popen([str(BIN), "serve", "--config", str(work / "config.json")], stdout=subprocess.DEVNULL, stderr=log)
    for _ in range(200):
        try:
            if subprocess.run(["curl", "-sf", f"http://127.0.0.1:{port}/health/live"], capture_output=True).returncode == 0:
                break
        except OSError:
            pass
        time.sleep(0.05)
    else:
        raise SystemExit("service did not start")
    return proc, port

def api(port, path, method="GET", body=None, attempts=5):
    last = ""
    for attempt in range(attempts):
        out = subprocess.run(
            ["curl", "-s", "-w", "\n%{http_code}", "-X", method, f"http://127.0.0.1:{port}{path}"] + (body or []),
            capture_output=True,
            text=True,
        ).stdout
        payload, _, code = out.rpartition("\n")
        if code in ("200", "201", "202", "204"):
            return payload.encode()
        last = f"{method} {path} -> {code}: {payload[:300]}"
        time.sleep(0.5 * (attempt + 1))
    raise RuntimeError(last)

def submit(port, path, name=None, options=None, attempts=6):
    last = ""
    for attempt in range(attempts):
        cmd = ["curl", "-s", "-w", "\n%{http_code}", "-F", f"file=@{path}"]
        if options:
            cmd += ["-F", f"options={json.dumps(options)}"]
        cmd.append(f"http://127.0.0.1:{port}/v1/extractions")
        out = subprocess.run(cmd, capture_output=True, text=True).stdout
        payload, _, code = out.rpartition("\n")
        if code == "202":
            return json.loads(payload)["id"]
        if code == "200":
            return json.loads(payload)["id"]  # cache hit
        last = f"upload {path} -> {code}: {payload[:300]}"
        time.sleep(0.5 * (attempt + 1))
    raise RuntimeError(last)

def status(port, jid):
    return json.loads(api(port, f"/v1/extractions/{jid}"))

def tree_rss(pid):
    rows = []
    for line in subprocess.check_output(["ps", "-axo", "pid=,ppid=,rss="], text=True).splitlines():
        try:
            p, pp, r = line.split()
            rows.append((int(p), int(pp), int(r)))
        except ValueError:
            pass
    ids = {pid}
    while True:
        grown = ids | {p for p, pp, _ in rows if pp in ids}
        if grown == ids:
            break
        ids = grown
    return sum(r for p, _, r in rows if p in ids) * 1024

def worker_pid(work, jid):
    try:
        return json.loads((work / "data/jobs" / jid / "worker.pid").read_text())
    except Exception:
        return None

def job_dir(work, jid):
    return work / "data/jobs" / jid

def run_job(work, port, path, options=None, name=None, timeout=600):
    started = time.time()
    jid = submit(port, path, name, options)
    peak = 0
    while True:
        st = status(port, jid)
        wp = worker_pid(work, jid)
        if wp:
            try:
                peak = max(peak, tree_rss(wp))
            except Exception:
                pass
        if st["state"] in ("completed", "failed", "cancelled") or time.time() - started > timeout:
            break
        time.sleep(0.1)
    wall = time.time() - started
    d = job_dir(work, jid)
    used = sum(f.stat().st_size for f in d.rglob("*") if f.is_file()) if d.exists() else 0
    timings = {}
    if (d / "timings.json").exists():
        timings = json.loads((d / "timings.json").read_text())
    result = d / "result.json"
    units = len(list((d / "units").glob("*.json"))) if (d / "units").exists() else 0
    return {
        "id": jid,
        "state": st["state"],
        "status": st.get("result_status"),
        "wall_s": round(wall, 2),
        "peak_rss_mb": round(peak / 1e6),
        "job_disk_mb": round(used / 1e6, 1),
        "units": units,
        "result_mb": round(result.stat().st_size / 1e6, 1) if result.exists() else 0,
        "timings": timings,
    }

def forget_cached(work, port, path, options=None):
    """Delete a completed job for this exact input+options so a phase runs on a fresh job."""
    jid = submit(port, path, options=options)
    if status(port, jid)["state"] in ("completed", "failed"):
        api(port, f"/v1/extractions/{jid}", method="DELETE", attempts=3)
        time.sleep(0.3)
    return jid

def recovery(work, limits, path, pages, service, port):
    """SIGKILL the service and the worker process group mid-run, restart, and measure resume.

    A *failed* service job is not re-used by cache lookups, so the meaningful crash path is the
    dispatcher: the job row stays `running`, and a restarted service re-spawns it into the same
    job directory, where completed units make Poppler work skip already-finished pages.
    """
    forget_cached(work, port, path)  # recovery.pdf is always run with default options
    jid = submit(port, path)
    d = job_dir(work, jid)
    deadline = time.time() + 180
    while time.time() < deadline:
        units = len(list((d / "units").glob("*.json"))) if (d / "units").exists() else 0
        if units >= max(3, pages // 4):
            break
        time.sleep(0.05)
    wp = worker_pid(work, jid)
    before = len(list((d / "units").glob("*.json"))) if (d / "units").exists() else 0
    # Kill the service first (SIGKILL, no chance to mark the job failed), then the worker group.
    service.kill()
    service.wait(timeout=15)
    killed = False
    if wp:
        try:
            os.killpg(wp, signal.SIGKILL)
            killed = True
        except (ProcessLookupError, PermissionError):
            pass
    time.sleep(0.5)
    new_service, new_port = start_service(work, limits)
    started = time.time()
    peak = 0
    st = {}
    while True:
        st = status(new_port, jid)
        np = worker_pid(work, jid)
        if np:
            try:
                peak = max(peak, tree_rss(np))
            except Exception:
                pass
        if st["state"] in ("completed", "failed") or time.time() - started > 300:
            break
        time.sleep(0.1)
    wall = time.time() - started
    timings = {}
    if (d / "timings.json").exists():
        timings = json.loads((d / "timings.json").read_text())
    final_units = len(list((d / "units").glob("*.json"))) if (d / "units").exists() else 0
    result = d / "result.json"
    evidence = {
        "job": jid,
        "units_before_kill": before,
        "worker_group_killed": killed,
        "resumed_state": st.get("state"),
        "resumed_status": st.get("result_status"),
        "resumed_wall_s": round(wall, 2),
        "peak_rss_mb": round(peak / 1e6),
        "final_units": final_units,
        "result_mb": round(result.stat().st_size / 1e6, 1) if result.exists() else 0,
        "timings": timings,
    }
    return evidence, new_service, new_port

def cancellation(work, port, path):
    forget_cached(work, port, path, options={"ocr": True})
    jid = submit(port, path, options={"ocr": True})
    d = job_dir(work, jid)
    for _ in range(200):
        if status(port, jid)["state"] == "running":
            break
        time.sleep(0.05)
    wp = worker_pid(work, jid)
    time.sleep(0.5)
    api(port, f"/v1/extractions/{jid}/cancel", method="POST", attempts=3)
    time.sleep(0.5)
    survivors = []
    if wp:
        out = subprocess.run(["ps", "-o", "pid=,command=", "-g", str(wp)], capture_output=True, text=True).stdout.strip()
        survivors = [l for l in out.splitlines() if l.strip()]
    temps = [f.name for f in d.iterdir() if ".tmp-" in f.name] if d.exists() else []
    return {"state": status(port, jid)["state"], "surviving_descendants": survivors, "temp_files": temps}

def main():
    phases = "runs,recovery,cancel"
    if "--phases" in sys.argv:
        phases = sys.argv[sys.argv.index("--phases") + 1]
    phases = set(phases.split(","))
    print(f"work dir: {WORK} phases: {sorted(phases)}")
    inputs = gen_inputs(WORK)
    limits = {
        "workers": 1,
        "queued_jobs": 4,
        "upload_bytes": 268435456,
        "output_bytes": 2147483648,
        "timeout_seconds": 1800,
        "memory_bytes": 6442450944,
        "pdf_workers": 4,
        "ocr_threads": 4,
        "stream_bytes": 16777216,
    }
    if "runs" in phases:
        shutil.rmtree(WORK / "data", ignore_errors=True)
    service, port = start_service(WORK, limits)
    evidence_path = WORK / "evidence.json"
    docs_path = ROOT / "docs/perf-large-inputs.json"
    evidence = {}
    if docs_path.exists():
        try:
            evidence = json.loads(docs_path.read_text())
        except Exception:
            evidence = {}
    if evidence_path.exists():
        evidence.update(json.loads(evidence_path.read_text()))
    runs = evidence.get("runs", {})
    try:
        if "runs" in phases:
            plan = [
                ("digital", WORK / "digital.pdf", None),
                ("scanned-ocr", WORK / "scanned.pdf", {"ocr": True}),
                ("mixed-ocr", WORK / "mixed.pdf", {"ocr": True}),
                ("image-heavy-ocr", WORK / "image-heavy.pdf", {"ocr": True}),
                ("csv-2m", WORK / "rows.csv", None),
                ("csv-2m-compact", WORK / "rows.csv", {"compact_tables": True}),
            ]
            for key, path, options in plan:
                runs[key] = run_job(WORK, port, path, options=options)
                evidence["runs"] = runs
                evidence_path.write_text(json.dumps(evidence, indent=1))
                print("run", key, json.dumps({k: v for k, v in runs[key].items() if k != "timings"}))
        if "recovery" in phases:
            evidence["recovery"], service, port = recovery(
                WORK, limits, WORK / "recovery.pdf", inputs["recovery.pdf"]["pages"], service, port
            )
            evidence_path.write_text(json.dumps(evidence, indent=1))
        if "cancel" in phases:
            evidence["cancellation"] = cancellation(WORK, port, WORK / "cancel.pdf")
            evidence_path.write_text(json.dumps(evidence, indent=1))
        recovery_result = evidence.get("recovery", {})
        cancel_result = evidence.get("cancellation", {})
    finally:
        service.send_signal(signal.SIGINT)
        try:
            service.wait(timeout=15)
        except subprocess.TimeoutExpired:
            service.kill()
    if QUICK:
        evidence["quick_mode"] = True
        inputs_note = "Quick mode: reduced page/row counts (CI runs use this)."
    else:
        inputs_note = "Full local shapes."
    evidence.update({
        "generated": time.strftime("%Y-%m-%d"),
        "inputs_note": inputs_note,
        "host": host_description(),
        "command": " ".join(sys.argv),
        "limits": limits,
        "inputs": inputs,
        "runs": runs,
        "recovery": recovery_result,
        "cancellation": cancel_result,
        "notes": [
            "peak_rss_mb is sampled process-tree RSS (100 ms); macOS keeps freed allocator pages resident, so it overstates steady-state usage. Streamed CSV live-heap peaks are recorded in docs/VALIDATION.md.",
            "timings come from the per-job timings.json (millisecond counters per stage, aggregated across batch workers).",
            "job_disk_mb covers the whole completed job directory including result files and retained assets.",
        ],
    })
    out = Path(os.environ.get("DOC_LARGE_BENCH_PATH", ROOT / "docs/perf-large-inputs.json"))
    out.write_text(json.dumps(evidence, indent=1) + "\n")
    print(json.dumps({k: {kk: vv for kk, vv in v.items() if kk != "timings"} for k, v in runs.items()}, indent=1))
    if recovery_result:
        print("recovery:", json.dumps({k: v for k, v in recovery_result.items() if k != "timings"}, indent=1))
    if cancel_result:
        print("cancellation:", json.dumps(cancel_result, indent=1))
    print("wrote", out)

if __name__ == "__main__":
    main()
