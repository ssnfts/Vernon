#!/usr/bin/env bash
# Installs the document-search embedding model for Vernon. Run inside the AI-3D WSL
# distribution as root:  bash install_docsearch.sh --yes [weights_dir]
# Idempotent: re-running reuses the environment and the downloaded weights.
set -euo pipefail
MODEL="BAAI/bge-m3"
LICENCE="MIT"
SIZE="about 2.3 GB weights (one weight format only) plus about 4 GB of Python packages (torch with CUDA)"
if [ "${1:-}" != "--yes" ]; then
  echo "Model:    $MODEL"
  echo "Licence:  $LICENCE"
  echo "Download: $SIZE"
  echo "Re-run with --yes to install."
  exit 1
fi
WEIGHTS="${2:-/mnt/e/AI-3D/DocSearch}"
ENV=/opt/ai3d/DocSearch
UV=/opt/uv/uv
PY=$ENV/bin/python
mkdir -p "$WEIGHTS" /opt/ai3d
[ -x "$PY" ] || $UV venv --python 3.11 "$ENV"
$UV pip install --python "$PY" torch sentence-transformers pypdf huggingface_hub numpy
export WEIGHTS
unset HF_HUB_OFFLINE || true
"$PY" - <<'PYEOF'
import os
from huggingface_hub import HfApi, snapshot_download
repo = "BAAI/bge-m3"
files = HfApi().list_repo_files(repo)
weight = "model.safetensors" if "model.safetensors" in files else "pytorch_model.bin"
patterns = [weight, "config.json", "config_sentence_transformers.json", "modules.json",
            "sentence_bert_config.json", "tokenizer.json", "tokenizer_config.json",
            "sentencepiece.bpe.model", "special_tokens_map.json", "1_Pooling/config.json"]
dest = os.path.join(os.environ["WEIGHTS"], "bge-m3")
info = HfApi().model_info(repo)
snapshot_download(repo, allow_patterns=patterns, local_dir=dest)
open(os.path.join(os.environ["WEIGHTS"], "model-revision.txt"), "w").write(info.sha + "\n")
print("weights", weight, "revision", info.sha)
PYEOF
$UV pip freeze --python "$PY" > "$WEIGHTS/installed-linux-packages.txt"
echo "done: $MODEL ($LICENCE) in $WEIGHTS"
