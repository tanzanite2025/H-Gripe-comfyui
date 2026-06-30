#!/usr/bin/env bash
# Fetch the bundled auto-subject model weight (U²-Netp, Apache-2.0) into the
# Tauri resources dir so `tauri build` can bundle it. Run before packaging a
# release. The weight is not committed to git (see resources/models/README.md).
set -euo pipefail

URL="https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx"
SHA256="309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_dir="$script_dir/../apps/desktop-tauri/src-tauri/resources/models"
dest="$dest_dir/u2netp.onnx"
mkdir -p "$dest_dir"

if [ -f "$dest" ] && command -v sha256sum >/dev/null 2>&1 \
  && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$SHA256" ]; then
  echo "u2netp.onnx already present and verified."
  exit 0
fi

echo "Downloading u2netp.onnx ..."
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
