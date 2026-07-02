# Fetch the Real-ESRGAN x4plus super-resolution weight (BSD-3-Clause) into the
# Tauri resources dir. This is a *downloadable big tier* (~64 MB): not bundled
# in the release by default. Run this to enable the `realesrgan` Image Enhance
# engine, or point HGRIPE_REALESRGAN_MODEL at a local copy for dev. The weight
# is not committed to git.
$ErrorActionPreference = 'Stop'

$Url = 'https://github.com/xinntao/Real-ESRGAN/releases/download/v0.1.0/RealESRGAN_x4plus.pth'
$Want = '4fa0d38905f75ac06eb49a7951b426670021be3018265fd191d2125df9d682f1'
$destDir = Join-Path $PSScriptRoot '..\apps\desktop-tauri\src-tauri\resources\models'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
$dest = Join-Path $destDir 'RealESRGAN_x4plus.pth'

if (Test-Path $dest) {
    $have = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    if ($have -eq $Want) {
        Write-Host 'RealESRGAN_x4plus.pth already present and verified.'
        return
    }
}
Write-Host 'Downloading RealESRGAN_x4plus.pth ...'
Invoke-WebRequest -Uri $Url -OutFile $dest
$got = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
if ($got -ne $Want) {
    Remove-Item $dest -Force
    throw "sha256 mismatch for RealESRGAN_x4plus.pth (got $got, want $Want)"
}
Write-Host "Fetched $dest"
