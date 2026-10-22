#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

PORT="${DOC_SMOLDOCLING_PORT:-8799}"

# Prefer native Apple Silicon MLX (mlx_vlm.server) when installed; otherwise use llama-server (GGUF).
if command -v mlx_vlm.server >/dev/null 2>&1; then
  MODEL="${DOC_SMOLDOCLING_MODEL:-ds4sd/SmolDocling-256M-preview-mlx-bf16}"
  echo "Starting SmolDocling-256M via Apple MLX ($MODEL) on http://127.0.0.1:$PORT ..."
  exec mlx_vlm.server --model "$MODEL" --port "$PORT"
fi

: "${DOC_VLM_WEIGHTS:=models/SmolDocling-256M-preview-Q8_0.gguf}"
: "${DOC_VLM_PROJECTOR:=models/mmproj-SmolDocling-256M-preview-Q8_0.gguf}"
if [ ! -f "$DOC_VLM_WEIGHTS" ] || [ ! -f "$DOC_VLM_PROJECTOR" ]; then
  echo "To run SmolDocling-256M locally (~350-500 MB RAM on Apple Silicon):" >&2
  echo "  Option A (Apple MLX):  pip install mlx-vlm && scripts/run-smoldocling.sh" >&2
  echo "  Option B (GGUF):       place $DOC_VLM_WEIGHTS and $DOC_VLM_PROJECTOR in models/" >&2
  exit 1
fi
exec llama-server -m "$DOC_VLM_WEIGHTS" --mmproj "$DOC_VLM_PROJECTOR" --host 127.0.0.1 --port "$PORT" -c 2048 -np 1 -ngl 99 --threads 2
