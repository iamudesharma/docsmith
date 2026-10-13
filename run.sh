#!/usr/bin/env bash
# One-command launcher for the doc-extractor backend.
#
#   ./run.sh                                  start on 127.0.0.1:8787 (builds first if needed)
#   DOC_PORT=9000 ./run.sh                    different port
#   DOC_WORKERS=4 ./run.sh                    concurrent jobs (isolated process per job)
#   DOC_DATA_DIR=/tmp/dsmith ./run.sh         runtime state location (jobs, units, results)
#   DOC_BIND=0.0.0.0:8787 DOC_TOKENS=secret1:tenant-a ./run.sh
#                                             non-loopback bind (tokens required)
#   DOC_REBUILD=1 ./run.sh                    force a fresh release build
#
# API: /health/live, /health/ready, /v1/capabilities, /openapi.json,
#      POST /v1/extractions (multipart file= + optional options=), GET/DELETE /v1/extractions/{id},
#      GET .../result?format=json|markdown|text, .../units/{unit_id}, .../assets/{asset_id},
#      POST .../cancel.  See docs/DEPLOYMENT.md and docs/CAPABILITIES.md.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

PORT="${DOC_PORT:-8787}"
BIND="${DOC_BIND:-127.0.0.1:$PORT}"
DATA_DIR="${DOC_DATA_DIR:-$ROOT/data/service}"
WORKERS="${DOC_WORKERS:-2}"
BIN="$ROOT/target/release/doc-extractor"

if [ "${DOC_REBUILD:-0}" = "1" ] || [ ! -x "$BIN" ]; then
  echo "== building release binary =="
  if [ "$(uname -s)" = "Darwin" ]; then
    "$ROOT/scripts/cargo-local.sh" build --release --locked
  else
    cargo build --release --locked
  fi
fi

case "$BIND" in
  127.0.0.1:*|localhost:*|\[::1\]:*)
    ;;
  *)
    if [ -z "${DOC_TOKENS:-}" ]; then
      echo "DOC_BIND=$BIND is not loopback. Set DOC_TOKENS=<token>:<tenant>[,<token>:<tenant>...]" >&2
      echo "or use the default loopback bind. See docs/DEPLOYMENT.md." >&2
      exit 1
    fi
    ;;
esac

mkdir -p "$DATA_DIR"
CONFIG="$DATA_DIR/config.json"
python3 - "$CONFIG" "$DATA_DIR" "$BIND" "$WORKERS" "${DOC_TOKENS:-}" <<'PY'
import json, sys
config_path, data_dir, bind, workers, tokens = sys.argv[1:6]
config = {"data_dir": data_dir, "bind": bind, "limits": {"workers": int(workers)}}
if tokens:
    config["tokens"] = {}
    for pair in tokens.split(","):
        pair = pair.strip()
        if not pair:
            continue
        token, _, tenant = pair.partition(":")
        config["tokens"][token] = tenant or "default"
json.dump(config, open(config_path, "w"), indent=2)
PY

echo "doc-extractor starting"
echo "  binary: $BIN"
echo "  config: $CONFIG"
echo "  state:  $DATA_DIR  (delete this directory to clear all cached jobs)"
echo "  API:    http://$BIND"
echo "  docs:   http://$BIND/docs  (Swagger UI: try every endpoint in the browser)"
echo "  guides: docs/DEPLOYMENT.md (ops), docs/CAPABILITIES.md (formats/options), docs/PIPELINE.md (concurrency)"
echo "  try:    curl -s http://$BIND/health/ready"
echo "          curl -s -F file=@fixtures/quoted.csv http://$BIND/v1/extractions"
echo "          curl -s http://$BIND/openapi.json"
echo "  stop:   Ctrl-C"
exec "$BIN" serve --config "$CONFIG"
