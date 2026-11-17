#!/usr/bin/env bash
# Installs the body-fit model (SAM 3D Body, DINOv3-H+ backbone) for Vernon. Run inside the
# AI-3D WSL distribution as root:
#   bash install_bodyfit.sh --yes --accept-licence [weights_dir]
# This model is opt-in. It is gated and is NOT under an open-source licence; run the script
# without flags to read what you agree to. Idempotent: re-running reuses the environment,
# the code checkouts and the downloaded weights.
set -euo pipefail
MODEL="facebook/sam-3d-body-dinov3"
MODEL_REV="11aaa346c7204874a1cbafe3d39a979080b2c55a"
CODE_REPO="https://github.com/facebookresearch/sam-3d-body"
CODE_REV="b5c765a0d89d789985e186d396315e7590887b94"
BACKBONE_REPO="https://github.com/facebookresearch/dinov3"
BACKBONE_REV="6876159a11b4df116f30f667f8c9888617df0751"
SIZE="about 2.7 GB of weights plus about 5.6 GB of Python packages (CUDA torch)"
if [ "${1:-}" != "--yes" ] || [ "${2:-}" != "--accept-licence" ]; then
  cat <<EOF
Model:    $MODEL (weights), $CODE_REPO (code)
Licence:  SAM License (Meta). Not open source. The backbone code ($BACKBONE_REPO)
          is under the DINOv3 License (Meta), also not open source.
Gate:     you must request access on the model's Hugging Face page with your own
          account, be approved, and sign in on this machine first:
            /opt/ai3d/Vision/bin/hf auth login
Terms you agree to (read the full texts on the pages above):
  * no military or warfare use, no nuclear, espionage or weapons use, nothing subject
    to arms export rules (ITAR);
  * if you pass the model or code on, it must go under the same licence, with a copy;
  * the model loads with Python pickle, which runs code from the weights file.
Download: $SIZE
Re-run with --yes --accept-licence to install.
EOF
  exit 1
fi
WEIGHTS="${3:-/mnt/e/AI-3D/BodyFit}"
ENV=/opt/ai3d/BodyFit
UV=/opt/uv/uv
PY=$ENV/bin/python
SRC=$ENV/src
mkdir -p "$WEIGHTS" /opt/ai3d
[ -x "$PY" ] || $UV venv --python 3.11 "$ENV"
# Only what inference on one whole picture needs: no person detector, no segmenter,
# no field-of-view estimator, no renderer. The second line is what the backbone's
# hubconf.py imports (its requirements.txt).
$UV pip install --python "$PY" torch torchvision numpy pillow opencv-python-headless roma \
  pytorch-lightning omegaconf yacs timm einops braceexpand huggingface_hub \
  ftfy regex scikit-learn submitit termcolor torchmetrics

# Pinned code checkouts. The bridge imports them from here; nothing is fetched at run time.
checkout() { # name url rev
  local dir="$SRC/$1"
  if [ "$(cat "$dir/COMMIT" 2>/dev/null || true)" != "$3" ]; then
    rm -rf "$dir"
    mkdir -p "$dir"
    git -C "$dir" init -q
    git -C "$dir" remote add origin "$2"
    git -C "$dir" -c http.version=HTTP/1.1 fetch -q --depth 1 origin "$3"
    git -C "$dir" checkout -q FETCH_HEAD
    echo "$3" > "$dir/COMMIT"
  fi
}
mkdir -p "$SRC"
checkout sam-3d-body "$CODE_REPO" "$CODE_REV"
checkout dinov3 "$BACKBONE_REPO" "$BACKBONE_REV"

# Download on the Linux disk first (a slow USB weights drive stalls the download),
# then copy the real files into the weights folder. HF_HUB_CACHE (not HF_HOME) keeps
# the saved sign-in visible.
STAGE=/var/tmp/bodyfit-stage
MODEL_DIR="$WEIGHTS/sam-3d-body-dinov3"
if [ ! -s "$MODEL_DIR/model.ckpt" ] || [ ! -s "$MODEL_DIR/assets/mhr_model.pt" ] || [ ! -s "$WEIGHTS/model-revision.txt" ]; then
  mkdir -p "$STAGE" "$MODEL_DIR/assets"
  HF_HUB_CACHE="$STAGE" HF_HUB_DISABLE_XET=1 HF_HUB_OFFLINE=0 WEIGHTS="$WEIGHTS" MODEL="$MODEL" MODEL_REV="$MODEL_REV" "$PY" - <<'PYEOF'
import os, shutil, sys
from huggingface_hub import HfApi, get_token, snapshot_download
repo, rev = os.environ["MODEL"], os.environ["MODEL_REV"]
if not get_token():
    sys.exit("not signed in to Hugging Face: run  /opt/ai3d/Vision/bin/hf auth login  and try again")
try:
    HfApi().auth_check(repo)
except Exception as e:
    sys.exit(f"this account cannot read {repo} ({type(e).__name__}): request access on its Hugging Face page and wait for approval")
p = snapshot_download(repo, revision=rev, allow_patterns=["model.ckpt", "model_config.yaml", "assets/mhr_model.pt"])
dst = os.path.join(os.environ["WEIGHTS"], "sam-3d-body-dinov3")
for f in ["model_config.yaml", "assets/mhr_model.pt", "model.ckpt"]:
    shutil.copyfile(os.path.join(p, f), os.path.join(dst, f + ".part"))
    os.replace(os.path.join(dst, f + ".part"), os.path.join(dst, f))
open(os.path.join(os.environ["WEIGHTS"], "model-revision.txt"), "w").write(rev + chr(10))
print("revision", rev)
PYEOF
  rm -rf "$STAGE"
fi
cp "$SRC/sam-3d-body/LICENSE" "$WEIGHTS/SAM-LICENSE.txt"
cp "$SRC/dinov3/LICENSE.md" "$WEIGHTS/DINOV3-LICENSE.md"
$UV pip freeze --python "$PY" > "$WEIGHTS/installed-linux-packages.txt"
echo "done: $MODEL (SAM License, opt-in) in $WEIGHTS"
