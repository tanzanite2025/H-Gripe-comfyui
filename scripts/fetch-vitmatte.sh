#!/usr/bin/env bash
# Fetch the ViTMatte (small) continuous-alpha matting weight (Apache-2.0) into
# the Tauri resources dir. This is a *downloadable big tier* (~104 MB): not
# bundled in the release by default. Run this to bundle it for a release, or
# point HGRIPE_VITMATTE_MODEL at a local copy for dev. The weight is not
# committed to git.
set -euo pipefail

URL="https://huggingface.co/Xenova/vitmatte-small-distinctions-646/resolve/main/onnx/model.onnx"
SHA256="a1cf48234c369faa3ea1711981d961fe1ec71f51e593f9d6553aa5a0e7d557e3"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_dir="$script_dir/../apps/desktop-tauri/src-tauri/resources/models"
mkdir -p "$dest_dir"
dest="$dest_dir/vitmatte.onnx"

if [ -f "$dest" ] && command -v sha256sum >/dev/null 2>&1 \
  && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$SHA256" ]; then
  echo "vitmatte.onnx already present and verified."
  exit 0
fi

echo "Downloading vitmatte.onnx ..."
curl -sSL -o "$dest" "$URL"
if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$dest" | cut -d' ' -f1)"
  if [ "$got" != "$SHA256" ]; then
    echo "ERROR: sha256 mismatch for vitmatte.onnx (got $got, want $SHA256)" >&2
    rm -f "$dest"
    exit 1
  fi
fi
echo "Fetched $dest"
