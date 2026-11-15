#!/usr/bin/env bash
# Installs the SAM 2.1 (Segment Anything) image model for Vernon. Run inside the
# AI-3D WSL distribution as root:  bash install_sam2.sh --yes [weights_dir]
# Idempotent: re-running reuses the environment and the downloaded weights.
set -euo pipefail
MODEL="facebook/sam2.1-hiera-large"
LICENCE="Apache-2.0"
SIZE="about 0.9 GB of weights plus about 6 GB of Python packages (CUDA torch)"
if [ "${1:-}" != "--yes" ]; then
  echo "Model:    $MODEL"
  echo "Licence:  $LICENCE"
  echo "Download: $SIZE"
  echo "Re-run with --yes to install."
  exit 1
fi
WEIGHTS="${2:-/mnt/e/AI-3D/SAM2}"
ENV=/opt/ai3d/SAM2
UV=/opt/uv/uv
PY=$ENV/bin/python
mkdir -p "$WEIGHTS" /opt/ai3d
[ -x "$PY" ] || $UV venv --python 3.11 "$ENV"
# transformers (5.x) carries the SAM 2 implementation; no separate sam2 package is needed.
$UV pip install --python "$PY" torch torchvision "transformers>=5" pillow numpy huggingface_hub
# Download on the Linux disk first (a slow USB weights drive stalls the download),
# then copy the real files into the weights folder.
STAGE=/var/tmp/sam2-stage
MODEL_DIR="$WEIGHTS/sam2.1-hiera-large"
if [ ! -s "$MODEL_DIR/model.safetensors" ] || [ ! -s "$WEIGHTS/model-revision.txt" ]; then
  mkdir -p "$STAGE" "$MODEL_DIR"
  HF_HOME="$STAGE" HF_HUB_DISABLE_XET=1 HF_HUB_OFFLINE=0 STAGE="$STAGE" WEIGHTS="$WEIGHTS" "$PY" - <<'PYEOF'
import os, shutil
from huggingface_hub import snapshot_download
p = snapshot_download("facebook/sam2.1-hiera-large",
                      allow_patterns=["config.json", "model.safetensors", "preprocessor_config.json", "processor_config.json"])
dst = os.path.join(os.environ["WEIGHTS"], "sam2.1-hiera-large")
for f in os.listdir(p):
    shutil.copyfile(os.path.join(p, f), os.path.join(dst, f + ".part"))
    os.replace(os.path.join(dst, f + ".part"), os.path.join(dst, f))
rev = os.path.basename(p)
open(os.path.join(os.environ["WEIGHTS"], "model-revision.txt"), "w").write(rev + chr(10))
print("revision", rev)
PYEOF
  rm -rf "$STAGE"
fi
$UV pip freeze --python "$PY" > "$WEIGHTS/installed-linux-packages.txt"
echo "done: $MODEL ($LICENCE) in $WEIGHTS"
