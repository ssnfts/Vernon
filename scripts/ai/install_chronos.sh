#!/usr/bin/env bash
# Installs the Chronos-2 forecasting model for Vernon's AI Convergence Forecast node.
# Run inside the Linux (WSL) environment as root:
#   bash install_chronos.sh --yes [--ai-root /mnt/e/AI-3D]
# Idempotent: re-running reuses the environment and skips files already downloaded.
set -euo pipefail

MODEL_REPO="amazon/chronos-2"
LICENCE="Apache-2.0"
SIZE="about 0.5 GB of weights, plus about 3 GB of Python packages (torch with CUDA)"
UV="${UV:-/opt/uv/uv}"
ENV_DIR="${VERNON_AI3D_LINUX_ROOT:-/opt/ai3d}/Chronos"
AI_ROOT="${VERNON_AI3D_LINUX_AI_ROOT:-/mnt/e/AI-3D}"

YES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --yes) YES=1 ;;
    --ai-root) shift; AI_ROOT="$1" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

echo "Model:    $MODEL_REPO"
echo "Licence:  $LICENCE"
echo "Download: $SIZE"
echo "Python:   $ENV_DIR"
echo "Weights:  $AI_ROOT/Chronos/model"
if [ "$YES" != 1 ]; then
  echo "Refusing to download without --yes." >&2
  exit 1
fi

DEST="$AI_ROOT/Chronos"
mkdir -p "$DEST"
[ -x "$ENV_DIR/bin/python" ] || "$UV" venv --python 3.11 "$ENV_DIR"
"$UV" pip install --python "$ENV_DIR/bin/python" torch "chronos-forecasting>=2,<3" huggingface_hub numpy

# Download the weights once into a plain folder (the bridge never downloads).
"$ENV_DIR/bin/python" - "$MODEL_REPO" "$DEST/model" "$DEST/model-revision.txt" <<'PY'
import sys
from huggingface_hub import snapshot_download, HfApi
repo, dest, revfile = sys.argv[1:4]
rev = HfApi().model_info(repo).sha
snapshot_download(repo_id=repo, revision=rev, local_dir=dest)
open(revfile, "w").write(rev + "\n")
print("revision", rev)
PY

"$UV" pip freeze --python "$ENV_DIR/bin/python" > "$DEST/installed-linux-packages.txt"
echo "done: $(cat "$DEST/model-revision.txt")"
