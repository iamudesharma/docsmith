#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
: "${DOC_VLM_WEIGHTS:=models/SmolVLM2-Q4_K_M.gguf}"
: "${DOC_VLM_PROJECTOR:=models/mmproj-SmolVLM2-Q8_0.gguf}"
[ -f "$DOC_VLM_WEIGHTS" ] && [ -f "$DOC_VLM_PROJECTOR" ] || { echo 'Provision weights and matching projector first; see docs/DEPLOYMENT.md' >&2; exit 1; }
exec llama-server -m "$DOC_VLM_WEIGHTS" --mmproj "$DOC_VLM_PROJECTOR" --host 127.0.0.1 --port 8799 -c 2048 -np 1 -ngl 99 --threads 2
