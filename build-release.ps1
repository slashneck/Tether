# Builds everything for a GitHub release into .\release:
#   TetherSetup-<version>.exe          the installer: the Windows app, Tether Camera and the phone app inside
#   TetherSetup-<version>.exe.sha256   its checksum, which the in-app updater verifies before running it
#   Tether-<version>.apk               the phone app on its own, to install straight on a phone
#   Tether-<version>.apk.sha256
#   Tether-<version>-source.zip        the source code (the committed tree)
#
# Needs: Rust (MSVC), the Android SDK + JDK 17+, the .NET 8 SDK, and the APK release key in
# %USERPROFILE%\.tether-keys\keystore.properties. The version comes from pc\Cargo.toml.

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Invoke-Checked([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE) { throw "$What failed" }
}

# Zip entries always use forward slashes, whatever the .NET version behind PowerShell does by default.
function New-Zip([string]$Folder, [string]$Destination) {
    $archive = [IO.Compression.ZipFile]::Open($Destination, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($file in Get-ChildItem $Folder -Recurse -File) {
            $name = $file.FullName.Substring($Folder.Length + 1).Replace('\', '/')
            [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, $name, [IO.Compression.CompressionLevel]::Optimal)
        }
    } finally {
        $archive.Dispose()
    }
}

function Write-Sha([string]$Path) {
    $name = Split-Path $Path -Leaf
    $sha = (Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$Path.sha256", "$sha  $name`n")
    return $sha
}

$version = ((Get-Content 'pc\Cargo.toml' -Raw) | Select-String '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"').Matches[0].Groups[1].Value
if (-not $version) { throw 'No version in pc\Cargo.toml' }
$apkVersion = ((Get-Content 'android\app\build.gradle.kts' -Raw) | Select-String 'versionName\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if ($apkVersion -ne $version) { throw "The phone app is $apkVersion but the PC app is $version. Set both to the same version." }
$keyProps = Join-Path $env:USERPROFILE '.tether-keys\keystore.properties'
if (-not $env:TETHER_KEYSTORE -and -not (Test-Path $keyProps)) { throw "The APK release key is missing ($keyProps)." }
Write-Host "Building Tether $version"

# 1. Windows app and Tether Camera
Push-Location pc
try { Invoke-Checked 'Building the Windows app' { cargo build --release } } finally { Pop-Location }

# 2. Phone app, signed with the release key
Push-Location android
try { Invoke-Checked 'Building the phone app' { .\gradlew.bat assembleRelease -q } } finally { Pop-Location }
$apk = 'android\app\build\outputs\apk\release\app-release.apk'

$out = Join-Path $PSScriptRoot 'release'
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
$app = Join-Path $out 'app'
New-Item -ItemType Directory (Join-Path $app 'adb') -Force | Out-Null
Copy-Item 'pc\target\release\tether.exe', 'pc\target\release\tether_vcam.dll' $app
Copy-Item $apk (Join-Path $app 'tether.apk')
$platformTools = Join-Path $env:LOCALAPPDATA 'Android\Sdk\platform-tools'
foreach ($f in 'adb.exe', 'AdbWinApi.dll', 'AdbWinUsbApi.dll', 'NOTICE.txt') { Copy-Item (Join-Path $platformTools $f) (Join-Path $app 'adb') }
Copy-Item 'LICENSE' (Join-Path $app 'LICENSE.txt')
Copy-Item 'THIRD-PARTY-NOTICES.md' $app

# 3. The uninstaller: the setup without an app inside
$uninstallOut = Join-Path $out 'uninstaller'
foreach ($dir in 'setup\bin', 'setup\obj') { if (Test-Path $dir) { Remove-Item $dir -Recurse -Force } }
Invoke-Checked 'Building the uninstaller' { dotnet build 'setup\Tether.Setup.csproj' -c Release "-p:Version=$version" -o $uninstallOut --nologo -v q }
Copy-Item (Join-Path $uninstallOut 'TetherSetup.exe') (Join-Path $app 'uninstall.exe')
Remove-Item $uninstallOut -Recurse -Force

# 4. The file list. Updates and uninstalling only ever touch the files named here.
$files = Get-ChildItem $app -Recurse -File | ForEach-Object { $_.FullName.Substring($app.Length + 1) } | Sort-Object
[IO.File]::WriteAllLines((Join-Path $app 'tether-files.txt'), [string[]]($files + 'tether-files.txt'), (New-Object Text.UTF8Encoding $false))

# 5. Setup, with everything inside
$payload = Join-Path $out 'payload.zip'
New-Zip $app $payload
$setupOut = Join-Path $out 'setup'
foreach ($dir in 'setup\bin', 'setup\obj') { if (Test-Path $dir) { Remove-Item $dir -Recurse -Force } }
Invoke-Checked 'Building the setup' { dotnet build 'setup\Tether.Setup.csproj' -c Release "-p:Version=$version" "-p:PayloadZip=$payload" -o $setupOut --nologo -v q }
$setup = Join-Path $out "TetherSetup-$version.exe"
Move-Item (Join-Path $setupOut 'TetherSetup.exe') $setup
$setupSha = Write-Sha $setup
Remove-Item $setupOut, $app, $payload -Recurse -Force
foreach ($dir in 'setup\bin', 'setup\obj') { if (Test-Path $dir) { Remove-Item $dir -Recurse -Force } }

# 6. The phone app on its own
$apkOut = Join-Path $out "Tether-$version.apk"
Copy-Item $apk $apkOut
$apkSha = Write-Sha $apkOut

# 7. Source code: exactly what's committed
Invoke-Checked 'Packing the source' { git archive --format=zip --prefix="Tether-$version-source/" -o (Join-Path $out "Tether-$version-source.zip") HEAD }

Write-Host ''
Write-Host "Done. Release files are in $out"
Write-Host "SHA-256  $setupSha  TetherSetup-$version.exe"
Write-Host "SHA-256  $apkSha  Tether-$version.apk"
Get-ChildItem $out | Format-Table Name, @{ Name = 'MB'; Expression = { [math]::Round($_.Length / 1MB, 1) } } -AutoSize
