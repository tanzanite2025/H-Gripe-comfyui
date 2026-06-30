#!/usr/bin/env bash
# Fetch the high-quality BiRefNet (lite) background-removal weight (MIT) into the
# Tauri resources dir. This is the *downloadable big tier* (~224 MB): unlike the
# small bundled u2netp default, it is not shipped in the release package by
# default. Run this to bundle it for a release, or point HGRIPE_BIREFNET_MODEL at
# a local copy for dev. The weight is not committed to git.
set -euo pipefail

URL="https://huggingface.co/onnx-community/BiRefNet_lite/resolve/main/onnx/model.onnx"
SHA256="5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_dir="$script_dir/../apps/desktop-tauri/src-tauri/resources/models"
dest="$dest_dir/birefnet_lite.onnx"
mkdir -p "$dest_dir"

if [ -f "$dest" ] && command -v sha256sum >/dev/null 2>&1 \
  && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$SHA256" ]; then
  echo "birefnet_lite.onnx already present and verified."
  exit 0
fi

echo "Downloading birefnet_lite.onnx (~224 MB) ..."
curl -sSL -o "$dest" "$URL"

if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$dest" | cut -d' ' -f1)"
  if [ "$got" != "$SHA256" ]; then
    echo "ERROR: sha256 mismatch (got $got, want $SHA256)" >&2
    rm -f "$dest"
    exit 1
  fi
fi
echo "Fetched $dest"
