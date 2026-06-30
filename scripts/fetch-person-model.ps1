# Fetch the auto_person human-segmentation weight (U²-Net human-seg, Apache-2.0)
# into the Tauri resources dir. This is a *downloadable* tier (~168 MB): like
# BiRefNet it is not shipped in the release package by default. The auto_person
# mode prefers it so a person matte tracks people rather than generic saliency;
# without it the card falls back to BiRefNet -> U2-Netp -> builtin-cpu. Run this
# to bundle it for a release, or point HGRIPE_PERSON_MODEL at a local copy for
# dev. The weight is not committed to git.
$ErrorActionPreference = 'Stop'

$Url = 'https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2net_human_seg.onnx'
$Sha256 = '01eb6a29a5c4d8edb30b56adad9bb3a2a0535338e480724a213e0acfd2d1c73c'

$destDir = Join-Path $PSScriptRoot '..\apps\desktop-tauri\src-tauri\resources\models'
$dest = Join-Path $destDir 'u2net_human_seg.onnx'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

if (Test-Path $dest) {
    $have = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    if ($have -eq $Sha256) {
        Write-Host 'u2net_human_seg.onnx already present and verified.'
        exit 0
    }
}

Write-Host 'Downloading u2net_human_seg.onnx (~168 MB) ...'
Invoke-WebRequest -Uri $Url -OutFile $dest

$got = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
if ($got -ne $Sha256) {
    Remove-Item $dest -Force
    throw "sha256 mismatch (got $got, want $Sha256)"
}
Write-Host "Fetched $dest"
