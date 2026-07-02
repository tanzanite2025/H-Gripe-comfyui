#!/usr/bin/env bash
# Fetch the Real-ESRGAN x4plus super-resolution weight (BSD-3-Clause) into the
# Tauri resources dir. This is a *downloadable big tier* (~64 MB): not bundled
# in the release by default. Run this to enable the `realesrgan` Image Enhance
# engine, or point HGRIPE_REALESRGAN_MODEL at a local copy for dev. The weight
# is not committed to git.
set -euo pipefail

URL="https://github.com/xinntao/Real-ESRGAN/releases/download/v0.1.0/RealESRGAN_x4plus.pth"
SHA256="4fa0d38905f75ac06eb49a7951b426670021be3018265fd191d2125df9d682f1"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_dir="$script_dir/../apps/desktop-tauri/src-tauri/resources/models"
mkdir -p "$dest_dir"
dest="$dest_dir/RealESRGAN_x4plus.pth"

if [ -f "$dest" ] && command -v sha256sum >/dev/null 2>&1 \
  && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$SHA256" ]; then
  echo "RealESRGAN_x4plus.pth already present and verified."
  exit 0
fi

echo "Downloading RealESRGAN_x4plus.pth ..."
curl -sSL -o "$dest" "$URL"
if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$dest" | cut -d' ' -f1)"
  if [ "$got" != "$SHA256" ]; then
    echo "ERROR: sha256 mismatch for RealESRGAN_x4plus.pth (got $got, want $SHA256)" >&2
    rm -f "$dest"
    exit 1
  fi
fi
echo "Fetched $dest"
