# Fetch the high-quality BiRefNet (lite) background-removal weight (MIT) into the
# Tauri resources dir. This is the *downloadable big tier* (~224 MB): unlike the
# small bundled u2netp default, it is not shipped in the release package by
# default. Run this to bundle it for a release, or point HGRIPE_BIREFNET_MODEL at
# a local copy for dev. The weight is not committed to git.
$ErrorActionPreference = 'Stop'

$Url = 'https://huggingface.co/onnx-community/BiRefNet_lite/resolve/main/onnx/model.onnx'
$Sha256 = '5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333'

$destDir = Join-Path $PSScriptRoot '..\apps\desktop-tauri\src-tauri\resources\models'
$dest = Join-Path $destDir 'birefnet_lite.onnx'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

if (Test-Path $dest) {
    $have = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    if ($have -eq $Sha256) {
        Write-Host 'birefnet_lite.onnx already present and verified.'
        exit 0
    }
}

Write-Host 'Downloading birefnet_lite.onnx (~224 MB) ...'
Invoke-WebRequest -Uri $Url -OutFile $dest

$got = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
if ($got -ne $Sha256) {
    Remove-Item $dest -Force
    throw "sha256 mismatch (got $got, want $Sha256)"
}
Write-Host "Fetched $dest"
