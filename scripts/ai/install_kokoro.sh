#!/usr/bin/env bash
# Installs the Kokoro text-to-speech model for Vernon. Run inside the AI-3D WSL
# distribution as root:  bash install_kokoro.sh --yes [weights_dir]
# Idempotent: re-running reuses the environment and the downloaded weights.
set -euo pipefail
MODEL="hexgrad/Kokoro-82M"
LICENCE="Apache-2.0"
SIZE="about 330 MB weights/voices plus about 1.5 GB of Python packages (CPU torch)"
if [ "${1:-}" != "--yes" ]; then
  echo "Model:    $MODEL"
  echo "Licence:  $LICENCE"
  echo "Download: $SIZE"
  echo "Re-run with --yes to install."
  exit 1
fi
WEIGHTS="${2:-/mnt/e/AI-3D/Kokoro}"
ENV=/opt/ai3d/Kokoro
UV=/opt/uv/uv
PY=$ENV/bin/python
mkdir -p "$WEIGHTS" /opt/ai3d
[ -x "$PY" ] || $UV venv --python 3.11 "$ENV"
$UV pip install --python "$PY" --extra-index-url https://download.pytorch.org/whl/cpu \
  "torch" "kokoro>=0.9.4" soundfile "misaki[en]" pip
# espeak-ng comes with the espeakng-loader wheel (pulled in by misaki[en]), so no
# system package is needed. The spaCy English model is installed here so the
# bridge never downloads anything.
"$PY" -c "import spacy.util as u; import sys; sys.exit(0 if u.is_package('en_core_web_sm') else 1)" 2>/dev/null \
  || $UV pip install --python "$PY" en_core_web_sm || \
  $UV pip install --python "$PY" "https://github.com/explosion/spacy-models/releases/download/en_core_web_sm-3.8.0/en_core_web_sm-3.8.0-py3-none-any.whl"
export HF_HOME="$WEIGHTS"
unset HF_HUB_OFFLINE || true
"$PY" - <<'PYEOF'
import json, os
from huggingface_hub import snapshot_download
VOICES = ["af_heart", "af_bella", "af_nicole", "am_michael", "bf_emma", "bm_george"]
p = snapshot_download("hexgrad/Kokoro-82M", allow_patterns=["config.json", "kokoro-v1_0.pth"] + [f"voices/{v}.pt" for v in VOICES])
rev = os.path.basename(p)
open(os.path.join(os.environ["HF_HOME"], "model-revision.txt"), "w").write(rev + "\n")
print("revision", rev)
PYEOF
$UV pip freeze --python "$PY" > "$WEIGHTS/installed-linux-packages.txt"
echo "done: $MODEL ($LICENCE) in $WEIGHTS"
