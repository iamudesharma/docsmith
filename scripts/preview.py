#!/usr/bin/env python3
"""Preview an extraction job against a running doc-extractor server.

The derived export is streamed, so previews of very large results stay cheap: only the
first lines are read and the connection is closed early.

    python3 scripts/preview.py JOB_ID                     # summary + first rows (text export)
    python3 scripts/preview.py JOB_ID --lines 40          # more rows
    python3 scripts/preview.py JOB_ID --markdown          # markdown export instead of text
    python3 scripts/preview.py JOB_ID --unit u3           # one chunk/unit table, first rows
    python3 scripts/preview.py JOB_ID --download out.json # save the full canonical JSON

Start the server with ./run.sh (default http://127.0.0.1:8787) or pass --base.
The Swagger UI at /docs can also run GET .../result with a Download button.
"""
import argparse
import json
import sys
import urllib.error
import urllib.request


def open_url(base, path):
    try:
        return urllib.request.urlopen(base + path, timeout=120)
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")
        sys.exit(f"HTTP {e.code} for {path}: {body[:200]}")


def unit_preview(unit, rows):
    print(f"unit {unit['id']}  kind={unit['kind']}  name={unit['name']}")
    for block in unit["blocks"][:3]:
        table = block.get("table")
        if table:
            print(f"  table {table['rows']} rows x {table['columns']} columns")
            cells = {}
            for cell in table["cells"]:
                cells.setdefault(cell["row"], {})[cell["column"]] = cell["raw"]
            for row in sorted(cells)[:rows]:
                print("   ", " | ".join(cells[row].get(i, "") for i in range(table["columns"])))
        elif block.get("text"):
            print("  text:", " ".join(block["text"].split())[:120])
        elif block.get("asset_id"):
            print(f"  asset: {block['asset_id']} ({block.get('mime', 'binary')})")
        else:
            print(" ", block["kind"])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("job", help="job id returned by POST /v1/extractions")
    ap.add_argument("--base", default="http://127.0.0.1:8787", help="server base URL")
    ap.add_argument("--lines", type=int, default=15, help="preview lines/rows (default 15)")
    ap.add_argument("--markdown", action="store_true", help="preview the markdown export instead of text")
    ap.add_argument("--unit", help="preview one unit/chunk (for example u1) instead of the whole result")
    ap.add_argument("--download", metavar="PATH", help="save the full canonical JSON result to PATH")
    args = ap.parse_args()
    base = args.base.rstrip("/")

    job = json.load(open_url(base, f"/v1/extractions/{args.job}"))
    print(f"{job['filename']}  state={job['state']}  progress={job.get('progress')}  result={job.get('result_status')}")
    for warning in job.get("warnings", []):
        print("  warning:", warning)
    for failure in job.get("unit_failures", []):
        print("  unit failure:", failure)
    if job["state"] != "completed":
        sys.exit(f"job is {job['state']}; poll: curl -s {base}/v1/extractions/{args.job}")

    if args.download:
        total = 0
        with open_url(base, f"/v1/extractions/{args.job}/result") as response, open(args.download, "wb") as out:
            while chunk := response.read(1 << 20):
                out.write(chunk)
                total += len(chunk)
        print(f"saved {total} bytes to {args.download}")

    if args.unit:
        print()
        unit_preview(json.load(open_url(base, f"/v1/extractions/{args.job}/units/{args.unit}")), args.lines)
    else:
        fmt = "markdown" if args.markdown else "text"
        print(f"\n--- first {args.lines} lines of result?format={fmt} ---")
        with open_url(base, f"/v1/extractions/{args.job}/result?format={fmt}") as response:
            for shown in range(args.lines):
                line = response.readline()
                if not line:
                    break
                sys.stdout.write(line.decode("utf-8", "replace"))

    print(
        "\n--- more ---\n"
        f"full JSON:  curl -s {base}/v1/extractions/{args.job}/result -o result.json\n"
        f"markdown:   curl -s '{base}/v1/extractions/{args.job}/result?format=markdown' | less\n"
        f"chunk:      curl -s {base}/v1/extractions/{args.job}/units/u1 | python3 -m json.tool | less\n"
        f"browser:    {base}/docs  (GET .../result -> Execute -> Download)"
    )


if __name__ == "__main__":
    main()
