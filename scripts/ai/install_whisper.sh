#!/usr/bin/env bash
# Installs Whisper large-v3-turbo for Vernon's "AI Text from Speech" node.
# Run inside the Linux environment as root:  bash install_whisper.sh --yes [WEIGHTS_DIR]
# Idempotent: re-running reuses the environment and skips files already downloaded.
set -euo pipefail

MODEL_REPO="openai/whisper-large-v3-turbo"
ENV_DIR="${VERNON_AI3D_LINUX_ROOT:-/opt/ai3d}/Whisper"
WEIGHTS="${2:-/mnt/e/AI-3D/Whisper}"
UV="${UV:-/opt/uv/uv}"

echo "Model:    $MODEL_REPO (809M parameters)"
echo "Licence:  MIT (model weights and the openai/whisper code)"
echo "Download: about 1.6 GB (model.safetensors, configs, tokenizer files only)"
echo "Python:   $ENV_DIR   Weights: $WEIGHTS"
if [ "${1:-}" != "--yes" ]; then
  echo "Refusing to run without --yes."
  exit 1
fi

[ -x "$ENV_DIR/bin/python" ] || "$UV" venv --python 3.11 "$ENV_DIR"
"$UV" pip install --python "$ENV_DIR/bin/python" torch transformers accelerate soundfile librosa huggingface_hub numpy

mkdir -p "$WEIGHTS/model" "$WEIGHTS/samples"
"$ENV_DIR/bin/python" - "$MODEL_REPO" "$WEIGHTS/model" "$WEIGHTS/download-whisper.json" <<'PY'
import json, sys
from huggingface_hub import snapshot_download, HfApi
repo, dest, record = sys.argv[1:4]
snapshot_download(repo, local_dir=dest, allow_patterns=[
    "model.safetensors", "config.json", "generation_config.json", "preprocessor_config.json",
    "tokenizer.json", "tokenizer_config.json", "vocab.json", "merges.txt",
    "normalizer.json", "added_tokens.json", "special_tokens_map.json"])
info = HfApi().model_info(repo)
json.dump({"repo": repo, "revision": info.sha, "license": "mit"}, open(record, "w"), indent=2)
print("revision", info.sha)
PY
"$UV" pip freeze --python "$ENV_DIR/bin/python" > "$WEIGHTS/installed-linux-packages.txt"
echo "done"
