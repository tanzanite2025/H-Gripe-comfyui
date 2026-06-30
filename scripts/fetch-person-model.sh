#!/usr/bin/env bash
# Fetch the auto_person human-segmentation weight (U²-Net human-seg, Apache-2.0)
# into the Tauri resources dir. This is a *downloadable* tier (~168 MB): like
# BiRefNet it is not shipped in the release package by default. The auto_person
# mode prefers it so a person matte tracks people rather than generic saliency;
# without it the card falls back to BiRefNet → U²-Netp → builtin-cpu. Run this to
# bundle it for a release, or point HGRIPE_PERSON_MODEL at a local copy for dev.
# The weight is not committed to git (see resources/models/README.md).
set -euo pipefail

URL="https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2net_human_seg.onnx"
SHA256="01eb6a29a5c4d8edb30b56adad9bb3a2a0535338e480724a213e0acfd2d1c73c"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_dir="$script_dir/../apps/desktop-tauri/src-tauri/resources/models"
dest="$dest_dir/u2net_human_seg.onnx"
mkdir -p "$dest_dir"

if [ -f "$dest" ] && command -v sha256sum >/dev/null 2>&1 \
  && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$SHA256" ]; then
  echo "u2net_human_seg.onnx already present and verified."
  exit 0
fi

echo "Downloading u2net_human_seg.onnx (~168 MB) ..."
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
