# Builds a test folder: dist\Tether\ with tether.exe, tether.apk and adb, ready to double-click.
# (The real installer comes with the release milestone; Tether Camera itself is installed with dev-install-vcam.ps1.)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

Push-Location (Join-Path $root 'pc')
try {
    cargo build --release
    if ($LASTEXITCODE) { throw 'cargo build failed' }
} finally { Pop-Location }

Push-Location (Join-Path $root 'android')
try {
    .\gradlew.bat assembleDebug -q
    if ($LASTEXITCODE) { throw 'gradle build failed' }
} finally { Pop-Location }

$out = Join-Path $root 'dist\Tether'
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory (Join-Path $out 'adb') -Force | Out-Null
Copy-Item (Join-Path $root 'pc\target\release\tether.exe') $out
Copy-Item (Join-Path $root 'android\app\build\outputs\apk\debug\app-debug.apk') (Join-Path $out 'tether.apk')
$pt = Join-Path $env:LOCALAPPDATA 'Android\Sdk\platform-tools'
foreach ($f in 'adb.exe', 'AdbWinApi.dll', 'AdbWinUsbApi.dll') { Copy-Item (Join-Path $pt $f) (Join-Path $out 'adb') }
Write-Host "Test build ready: $out\tether.exe"
