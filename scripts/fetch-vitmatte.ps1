# Fetch the ViTMatte (small) continuous-alpha matting weight (Apache-2.0) into
# the Tauri resources dir. This is a *downloadable big tier* (~104 MB): not
# bundled in the release by default. Run this to bundle it for a release, or
# point HGRIPE_VITMATTE_MODEL at a local copy for dev. The weight is not
# committed to git.
$ErrorActionPreference = 'Stop'

$Url = 'https://huggingface.co/Xenova/vitmatte-small-distinctions-646/resolve/main/onnx/model.onnx'
$Want = 'a1cf48234c369faa3ea1711981d961fe1ec71f51e593f9d6553aa5a0e7d557e3'
$destDir = Join-Path $PSScriptRoot '..\apps\desktop-tauri\src-tauri\resources\models'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
$dest = Join-Path $destDir 'vitmatte.onnx'

if (Test-Path $dest) {
    $have = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    if ($have -eq $Want) {
        Write-Host 'vitmatte.onnx already present and verified.'
        return
    }
}
Write-Host 'Downloading vitmatte.onnx ...'
Invoke-WebRequest -Uri $Url -OutFile $dest
$got = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
if ($got -ne $Want) {
    Remove-Item $dest -Force
    throw "sha256 mismatch for vitmatte.onnx (got $got, want $Want)"
}
Write-Host "Fetched $dest"
