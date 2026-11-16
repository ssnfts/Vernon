#!/usr/bin/env bash
# Installs FLUX.2-klein-4B for Vernon's "AI Sketch from Words" node.
# Run inside the Linux environment as root:  bash install_sketch.sh --yes [WEIGHTS_DIR]
# Idempotent: re-running reuses the environment and skips files already downloaded.
# The download goes to the Linux disk first and is copied to WEIGHTS_DIR afterwards
# (downloads straight onto the Windows drive through WSL have stalled before). Big
# files are checked by sha256 against the hashes published by Hugging Face.
set -euo pipefail

MODEL_REPO="black-forest-labs/FLUX.2-klein-4B"
ENV_DIR="${VERNON_AI3D_LINUX_ROOT:-/opt/ai3d}/Sketch"
WEIGHTS="${2:-/mnt/e/AI-3D/Sketch}"
UV="${UV:-/opt/uv/uv}"
DL="$ENV_DIR/dl"

echo "Model:    $MODEL_REPO (4B parameter image model, bfloat16)"
echo "Licence:  Apache-2.0 (checked against the model repository card, not gated)"
echo "Download: about 16 GB (transformer, text encoder and VAE weights, configs and tokenizer; the duplicate single-file copy is skipped)"
echo "Python:   $ENV_DIR   Weights: $WEIGHTS"
if [ "${1:-}" != "--yes" ]; then
  echo "Refusing to run without --yes."
  exit 1
fi

[ -x "$ENV_DIR/bin/python" ] || "$UV" venv --python 3.11 "$ENV_DIR"
"$UV" pip install --python "$ENV_DIR/bin/python" torch transformers accelerate "diffusers>=0.37" pillow numpy huggingface_hub safetensors

mkdir -p "$DL" "$WEIGHTS/model"
"$ENV_DIR/bin/python" - "$MODEL_REPO" "$DL" "$WEIGHTS" <<'PY'
import hashlib, json, os, shutil, sys
from pathlib import Path
from huggingface_hub import snapshot_download, HfApi
repo, dl, root = sys.argv[1:4]
os.environ.pop("HF_HUB_OFFLINE", None)
api = HfApi()
info = api.model_info(repo, files_metadata=True)
card = info.card_data.to_dict() if info.card_data else {}
if card.get("license") != "apache-2.0" or info.gated:
    sys.exit(f"licence check failed: license={card.get('license')} gated={info.gated}")
patterns = ["model_index.json", "scheduler/*", "text_encoder/*", "tokenizer/*", "transformer/*", "vae/*"]
for attempt in range(5):
    try:
        snapshot_download(repo, local_dir=dl, allow_patterns=patterns, revision=info.sha)
        break
    except Exception as e:  # noqa
        print("download retry:", type(e).__name__, e, flush=True)
else:
    sys.exit("download failed")
want = {s.rfilename: s.lfs.sha256 for s in info.siblings if s.lfs and any(s.rfilename.startswith(p.rstrip("*")) for p in patterns[1:])}
def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        while b := f.read(64 << 20):
            h.update(b)
    return h.hexdigest()
dest = Path(root) / "model"
for name, digest in want.items():
    src = Path(dl) / name
    if sha(src) != digest:
        sys.exit(f"sha256 mismatch in download: {name}")
    out = dest / name
    out.parent.mkdir(parents=True, exist_ok=True)
    if not (out.is_file() and out.stat().st_size == src.stat().st_size and sha(out) == digest):
        shutil.copyfile(src, out)
        if sha(out) != digest:
            sys.exit(f"sha256 mismatch after copy: {name}")
    print("verified", name, flush=True)
for f in ["model_index.json"] + [str(p.relative_to(dl)) for p in Path(dl, "").rglob("*") if p.is_file() and not str(p.relative_to(dl)).startswith(".cache") and str(p.relative_to(dl)) not in want]:
    if f.startswith(".cache"):
        continue
    out = dest / f
    out.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(Path(dl) / f, out)
(Path(root) / "model-revision.txt").write_text(info.sha + "\n")
print("revision", info.sha)
PY
"$UV" pip freeze --python "$ENV_DIR/bin/python" > "$WEIGHTS/installed-linux-packages.txt"
echo "done"
