#!/usr/bin/env bash
# Fetch the SAM 2 (tiny) interactive point-prompt weights (Apache-2.0) into the
# Tauri resources dir. This is a *downloadable big tier* (encoder ~134 MB +
# decoder ~20 MB): not bundled in the release by default. Run this to bundle
# them for a release, or point HGRIPE_SAM2_ENCODER / HGRIPE_SAM2_DECODER at
# local copies for dev. The weights are not committed to git.
set -euo pipefail

BASE="https://huggingface.co/vietanhdev/segment-anything-2-onnx-models/resolve/main"
ENCODER_URL="$BASE/sam2_hiera_tiny.encoder.onnx"
DECODER_URL="$BASE/sam2_hiera_tiny.decoder.onnx"
ENCODER_SHA256="4cc015ee18520e93f8c7ddfeaca7436039daaaaf19721b4b96a8810a805e82f7"
DECODER_SHA256="f5a4bd656c143899fb7f52d64ed81e6f6aeb37d477a0b6da50146ac7cf2187bf"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_dir="$script_dir/../apps/desktop-tauri/src-tauri/resources/models"
mkdir -p "$dest_dir"

fetch() {
  local url="$1" name="$2" want="$3" dest="$dest_dir/$2"
  if [ -f "$dest" ] && command -v sha256sum >/dev/null 2>&1 \
    && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$want" ]; then
    echo "$name already present and verified."
    return 0
  fi
  echo "Downloading $name ..."
  curl -sSL -o "$dest" "$url"
  if command -v sha256sum >/dev/null 2>&1; then
    local got
    got="$(sha256sum "$dest" | cut -d' ' -f1)"
    if [ "$got" != "$want" ]; then
      echo "ERROR: sha256 mismatch for $name (got $got, want $want)" >&2
      rm -f "$dest"
      exit 1
    fi
  fi
  echo "Fetched $dest"
}

fetch "$ENCODER_URL" "sam2_tiny.encoder.onnx" "$ENCODER_SHA256"
fetch "$DECODER_URL" "sam2_tiny.decoder.onnx" "$DECODER_SHA256"
