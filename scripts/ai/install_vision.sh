#!/usr/bin/env bash
# Installs Qwen3-VL-4B-Instruct for Vernon's "AI Vision Check" node.
# Run inside the Linux environment as root:  bash install_vision.sh --yes [WEIGHTS_DIR]
# Idempotent: re-running reuses the environment and skips files already downloaded.
set -euo pipefail

MODEL_REPO="Qwen/Qwen3-VL-4B-Instruct"
ENV_DIR="${VERNON_AI3D_LINUX_ROOT:-/opt/ai3d}/Vision"
WEIGHTS="${2:-/mnt/e/AI-3D/Vision}"
UV="${UV:-/opt/uv/uv}"

echo "Model:    $MODEL_REPO (about 4B parameters, bfloat16)"
echo "Licence:  Apache-2.0 (checked against the model repository tags)"
echo "Download: about 9 GB (safetensors weights, configs, tokenizer and processor files only)"
echo "Python:   $ENV_DIR   Weights: $WEIGHTS"
if [ "${1:-}" != "--yes" ]; then
  echo "Refusing to run without --yes."
  exit 1
fi

[ -x "$ENV_DIR/bin/python" ] || "$UV" venv --python 3.11 "$ENV_DIR"
"$UV" pip install --python "$ENV_DIR/bin/python" torch torchvision transformers accelerate pillow numpy huggingface_hub

mkdir -p "$WEIGHTS/model"
"$ENV_DIR/bin/python" - "$MODEL_REPO" "$WEIGHTS/model" "$WEIGHTS" <<'PY'
import json, os, sys
from huggingface_hub import snapshot_download, HfApi
repo, dest, root = sys.argv[1:4]
os.environ.pop("HF_HUB_OFFLINE", None)
snapshot_download(repo, local_dir=dest, allow_patterns=[
    "*.safetensors", "model.safetensors.index.json", "config.json", "generation_config.json",
    "preprocessor_config.json", "video_preprocessor_config.json", "chat_template.json",
    "tokenizer.json", "tokenizer_config.json", "vocab.json", "merges.txt"])
info = HfApi().model_info(repo)
tags = [t for t in info.tags if t.startswith("license:")]
if tags != ["license:apache-2.0"]:
    sys.exit(f"unexpected licence tags {tags}")
open(os.path.join(root, "model-revision.txt"), "w").write(info.sha + "\n")
print("revision", info.sha)
PY
"$UV" pip freeze --python "$ENV_DIR/bin/python" > "$WEIGHTS/installed-linux-packages.txt"
echo "done"
