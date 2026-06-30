# Fetch the SAM 2 (tiny) interactive point-prompt weights (Apache-2.0) into the
# Tauri resources dir. This is a *downloadable big tier* (encoder ~134 MB +
# decoder ~20 MB): not bundled in the release by default. Run this to bundle
# them for a release, or point HGRIPE_SAM2_ENCODER / HGRIPE_SAM2_DECODER at
# local copies for dev. The weights are not committed to git.
$ErrorActionPreference = 'Stop'

$Base = 'https://huggingface.co/vietanhdev/segment-anything-2-onnx-models/resolve/main'
$destDir = Join-Path $PSScriptRoot '..\apps\desktop-tauri\src-tauri\resources\models'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

function Fetch-Weight($Url, $Name, $Want) {
    $dest = Join-Path $destDir $Name
    if (Test-Path $dest) {
        $have = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
        if ($have -eq $Want) {
            Write-Host "$Name already present and verified."
            return
        }
    }
    Write-Host "Downloading $Name ..."
    Invoke-WebRequest -Uri $Url -OutFile $dest
    $got = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    if ($got -ne $Want) {
        Remove-Item $dest -Force
        throw "sha256 mismatch for $Name (got $got, want $Want)"
    }
    Write-Host "Fetched $dest"
}

Fetch-Weight "$Base/sam2_hiera_tiny.encoder.onnx" 'sam2_tiny.encoder.onnx' '4cc015ee18520e93f8c7ddfeaca7436039daaaaf19721b4b96a8810a805e82f7'
Fetch-Weight "$Base/sam2_hiera_tiny.decoder.onnx" 'sam2_tiny.decoder.onnx' 'f5a4bd656c143899fb7f52d64ed81e6f6aeb37d477a0b6da50146ac7cf2187bf'
