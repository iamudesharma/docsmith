#!/usr/bin/env python3
"""Minimal dependency-free client for the doc-extractor HTTP service.

    from docsmith import Client

    with Client("http://127.0.0.1:8787") as client:
        job = client.extract_and_wait("report.pdf", options={"ocr": True})
        print(client.text(job["id"])[:500])

    # Stream a large result without loading it into memory:
    with open("out.txt", "wb") as out:
        for chunk in client.stream(job["id"], "text"):
            out.write(chunk)

Command line:

    python3 docsmith.py extract report.pdf --text
    python3 docsmith.py list --state completed
    python3 docsmith.py stats
"""
from __future__ import annotations

import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

__all__ = ["Client", "ApiError"]


class ApiError(RuntimeError):
    """Non-2xx response; `status` and decoded `payload` are preserved."""

    def __init__(self, status: int, payload):
        super().__init__(f"HTTP {status}: {payload}")
        self.status = status
        self.payload = payload


class Client:
    def __init__(self, base_url: str = "http://127.0.0.1:8787", token: str | None = None, timeout: float = 120):
        self.base_url = base_url.rstrip("/")
        self.token = token or os.environ.get("DOC_TOKEN")
        self.timeout = timeout

    # ------------------------------------------------------------------ plumbing
    def _request(self, path: str, method: str = "GET", body: bytes | None = None, headers: dict | None = None, stream: bool = False):
        request = urllib.request.Request(self.base_url + path, data=body, method=method)
        for key, value in (headers or {}).items():
            request.add_header(key, value)
        if self.token:
            request.add_header("Authorization", "Bearer " + self.token)
        try:
            response = urllib.request.urlopen(request, timeout=self.timeout)
        except urllib.error.HTTPError as error:
            payload = error.read().decode("utf-8", "replace")
            try:
                payload = json.loads(payload)
            except ValueError:
                pass
            raise ApiError(error.code, payload) from None
        return response if stream else response.read()

    def _json(self, path: str, method: str = "GET", body: bytes | None = None, headers: dict | None = None):
        raw = self._request(path, method, body, headers)
        return json.loads(raw) if raw else None

    # ------------------------------------------------------------------ API
    def capabilities(self):
        """Formats and optional OCR/vision providers configured on the server."""
        return self._json("/v1/capabilities")

    def stats(self):
        """Queue depth, job counts and disk usage for the caller's tenant."""
        return self._json("/v1/stats")

    def list(self, state: str | None = None, limit: int = 50, offset: int = 0):
        query = f"?limit={limit}&offset={offset}" + (f"&state={state}" if state else "")
        return self._json("/v1/extractions" + query)

    def extract(self, path: str | Path | None = None, data: bytes | None = None, filename: str | None = None,
                options: dict | None = None, callback_url: str | None = None):
        """Upload one document; returns the 202 dict (`cached` means a stored result was reused)."""
        if path is not None:
            data = Path(path).read_bytes()
            filename = filename or Path(path).name
        if data is None or filename is None:
            raise ValueError("extract() needs path= or data= plus filename=")
        boundary = "docsmith-" + os.urandom(8).hex()
        parts = [
            f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{filename}"\r\n'
            f"Content-Type: application/octet-stream\r\n\r\n".encode(),
            data,
            b"\r\n",
        ]
        if options is not None:
            parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="options"\r\n\r\n'.encode())
            parts.append(json.dumps(options).encode())
            parts.append(b"\r\n")
        if callback_url:
            parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="callback_url"\r\n\r\n'.encode())
            parts.append(callback_url.encode())
            parts.append(b"\r\n")
        parts.append(f"--{boundary}--\r\n".encode())
        return self._json(
            "/v1/extractions",
            "POST",
            b"".join(parts),
            {"Content-Type": f"multipart/form-data; boundary={boundary}"},
        )

    def wait(self, job_id: str, timeout: float = 3600, interval: float = 0.5, on_progress=None):
        """Poll until the job is terminal; raises TimeoutError instead of polling forever."""
        deadline = time.time() + timeout
        while True:
            job = self.status(job_id)
            if on_progress:
                on_progress(job)
            if job["state"] in ("completed", "failed", "cancelled"):
                return job
            if time.time() > deadline:
                raise TimeoutError(f"job {job_id} still {job['state']} after {timeout}s")
            time.sleep(interval)

    def extract_and_wait(self, path=None, data=None, filename=None, options=None, callback_url=None,
                         timeout: float = 3600, interval: float = 0.5, on_progress=None):
        job = self.extract(path=path, data=data, filename=filename, options=options, callback_url=callback_url)
        return self.wait(job["id"], timeout=timeout, interval=interval, on_progress=on_progress)

    def status(self, job_id: str):
        return self._json(f"/v1/extractions/{job_id}")

    def cancel(self, job_id: str):
        return self._json(f"/v1/extractions/{job_id}/cancel", "POST", b"")

    def delete(self, job_id: str):
        """Cancel and remove the job plus its retained files."""
        return self._request(f"/v1/extractions/{job_id}", "DELETE")

    def result(self, job_id: str, format: str | None = None) -> bytes:
        query = f"?format={format}" if format else ""
        return self._request(f"/v1/extractions/{job_id}/result{query}")

    def text(self, job_id: str) -> str:
        return self.result(job_id, "text").decode("utf-8")

    def markdown(self, job_id: str) -> str:
        return self.result(job_id, "markdown").decode("utf-8")

    def jsonl(self, job_id: str) -> str:
        return self.result(job_id, "jsonl").decode("utf-8")

    def document(self, job_id: str):
        return json.loads(self.result(job_id, "json"))

    def stream(self, job_id: str, format: str = "text", chunk_size: int = 65536):
        """Yield result bytes as they arrive (nothing is buffered in full)."""
        response = self._request(f"/v1/extractions/{job_id}/result?format={format}", stream=True)
        with response:
            while True:
                chunk = response.read(chunk_size)
                if not chunk:
                    break
                yield chunk

    def download(self, job_id: str, destination: str | Path, format: str = "text"):
        with open(destination, "wb") as out:
            for chunk in self.stream(job_id, format):
                out.write(chunk)
        return Path(destination)

    def units(self, job_id: str, unit_id: str, rows: int | None = None):
        """One unit/chunk; `rows` returns a cheap `{slice, unit}` preview of a large table."""
        query = f"?count={rows}" if rows else ""
        return self._json(f"/v1/extractions/{job_id}/units/{unit_id}{query}")

    def archive(self, job_id: str, destination: str | Path | None = None, units: bool = False):
        """Zip of result artifacts; `units=True` also includes unit checkpoints."""
        data = self._request(f"/v1/extractions/{job_id}/archive{'?units=1' if units else ''}")
        if destination:
            Path(destination).write_bytes(data)
            return Path(destination)
        return data

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        return False


def _main():
    import argparse

    parser = argparse.ArgumentParser(description="doc-extractor client")
    parser.add_argument("--base", default=os.environ.get("DOC_BASE_URL", "http://127.0.0.1:8787"))
    parser.add_argument("--token", default=None)
    commands = parser.add_subparsers(dest="command", required=True)
    extract = commands.add_parser("extract", help="upload a file and wait for completion")
    extract.add_argument("file")
    extract.add_argument("--options", help="JSON options string, for example '{\"ocr\": true}'")
    extract.add_argument("--text", action="store_true", help="print the text result")
    listing = commands.add_parser("list", help="list jobs")
    listing.add_argument("--state")
    commands.add_parser("stats", help="print /v1/stats")
    args = parser.parse_args()

    with Client(args.base, token=args.token) as client:
        if args.command == "stats":
            print(json.dumps(client.stats(), indent=2))
        elif args.command == "list":
            print(json.dumps(client.list(state=args.state), indent=2))
        else:
            options = json.loads(args.options) if args.options else None
            job = client.extract_and_wait(
                args.file,
                options=options,
                on_progress=lambda j: print(f"{j['state']} {j['progress']}", file=sys.stderr),
            )
            print(json.dumps(job, indent=2))
            if args.text:
                print(client.text(job["id"]), end="")


if __name__ == "__main__":
    _main()
