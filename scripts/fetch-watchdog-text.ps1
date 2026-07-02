# Fetch the PP-OCRv3 English text-detection weight (PaddleOCR, Apache-2.0,
# ~2.4 MB ONNX export) into the Tauri resources dir, plus its label-map sidecar.
# Run this to graduate the Detail Watchdog `text` watch target from `skipped`
# to real findings under the `onnx_defect` engine, or point
# HGRIPE_WATCHDOG_MODEL at a local copy for dev. The weight is not committed to
# git.
$ErrorActionPreference = 'Stop'

$Url = 'https://huggingface.co/deepghs/paddleocr/resolve/main/det/en_PP-OCRv3_det/model.onnx'
$Want = '69d10a2f151e0561e7e6c948ff0207a5fb84789fa6a4591d1d08138e3d82f1f9'
$destDir = Join-Path $PSScriptRoot '..\apps\desktop-tauri\src-tauri\resources\models'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
$dest = Join-Path $destDir 'watchdog_defect.onnx'

# The sidecar tells the backend what the weight covers (`text` only — the
# report keeps hands/logo truthfully `skipped`) and that it wants ImageNet
# input normalisation (the PaddleOCR convention).
Set-Content -Path "$dest.labels.json" -NoNewline `
    -Value '{"labels": {"0": "text"}, "normalize": "imagenet"}'

if (Test-Path $dest) {
    $have = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    if ($have -eq $Want) {
        Write-Host 'watchdog_defect.onnx already present and verified.'
        return
    }
}
Write-Host 'Downloading watchdog_defect.onnx (PP-OCRv3 det) ...'
Invoke-WebRequest -Uri $Url -OutFile $dest
$got = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
if ($got -ne $Want) {
    Remove-Item $dest -Force
    throw "sha256 mismatch for watchdog_defect.onnx (got $got, want $Want)"
}
Write-Host "Fetched $dest"
