#!/usr/bin/env bash
# Fetch the PP-OCRv3 English text-detection weight (PaddleOCR, Apache-2.0,
# ~2.4 MB ONNX export) into the Tauri resources dir, plus its label-map sidecar.
# Run this to graduate the Detail Watchdog `text` watch target from `skipped`
# to real findings under the `onnx_defect` engine, or point
# HGRIPE_WATCHDOG_MODEL at a local copy for dev. The weight is not committed to
# git.
set -euo pipefail

URL="https://huggingface.co/deepghs/paddleocr/resolve/main/det/en_PP-OCRv3_det/model.onnx"
SHA256="69d10a2f151e0561e7e6c948ff0207a5fb84789fa6a4591d1d08138e3d82f1f9"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_dir="$script_dir/../apps/desktop-tauri/src-tauri/resources/models"
mkdir -p "$dest_dir"
dest="$dest_dir/watchdog_defect.onnx"

# The sidecar tells the backend what the weight covers (`text` only — the
# report keeps hands/logo truthfully `skipped`) and that it wants ImageNet
# input normalisation (the PaddleOCR convention).
printf '%s\n' '{"labels": {"0": "text"}, "normalize": "imagenet"}' \
  > "$dest.labels.json"

if [ -f "$dest" ] && command -v sha256sum >/dev/null 2>&1 \
  && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$SHA256" ]; then
  echo "watchdog_defect.onnx already present and verified."
  exit 0
fi

echo "Downloading watchdog_defect.onnx (PP-OCRv3 det) ..."
curl -sSL -o "$dest" "$URL"
if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$dest" | cut -d' ' -f1)"
  if [ "$got" != "$SHA256" ]; then
    echo "ERROR: sha256 mismatch for watchdog_defect.onnx (got $got, want $SHA256)" >&2
    rm -f "$dest"
    exit 1
  fi
fi
echo "Fetched $dest"
