# Fetch the bundled auto-subject model weight (U2-Netp, Apache-2.0) into the
# Tauri resources dir so `tauri build` can bundle it. Run before packaging a
# release. The weight is not committed to git (see resources/models/README.md).
$ErrorActionPreference = 'Stop'

$Url = 'https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx'
$Sha256 = '309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8'

$destDir = Join-Path $PSScriptRoot '..\apps\desktop-tauri\src-tauri\resources\models'
$dest = Join-Path $destDir 'u2netp.onnx'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

if (Test-Path $dest) {
    $have = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    if ($have -eq $Sha256) {
        Write-Host 'u2netp.onnx already present and verified.'
        exit 0
    }
}

Write-Host 'Downloading u2netp.onnx ...'
Invoke-WebRequest -Uri $Url -OutFile $dest

$got = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
if ($got -ne $Sha256) {
    Remove-Item $dest -Force
    throw "sha256 mismatch (got $got, want $Sha256)"
}
Write-Host "Fetched $dest"
