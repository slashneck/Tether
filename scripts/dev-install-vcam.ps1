# Developer helper: installs the freshly built camera DLL where Windows' camera service can read it, and registers it.
# Asks for admin (the camera service only reads machine-wide COM registrations).
#
#   .\scripts\dev-install-vcam.ps1            build (release) + install + register
#   .\scripts\dev-install-vcam.ps1 -Uninstall unregister + remove

param([switch]$Uninstall, [switch]$Debug)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$dest = Join-Path $env:ProgramFiles 'Tether\dev'
$dll = Join-Path $dest 'tether_vcam.dll'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $Uninstall) {
    $profileName = if ($Debug) { 'debug' } else { 'release' }
    if (-not $isAdmin) {
        Push-Location (Join-Path $root 'pc')
        try {
            if ($Debug) { cargo build -p tether-vcam } else { cargo build -p tether-vcam --release }
            if ($LASTEXITCODE) { throw 'cargo build failed' }
        } finally { Pop-Location }
    }
    $built = Join-Path $root "pc\target\$profileName\tether_vcam.dll"
}

if (-not $isAdmin) {
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
    if ($Uninstall) { $argList += '-Uninstall' }
    if ($Debug) { $argList += '-Debug' }
    $p = Start-Process powershell -Verb RunAs -ArgumentList $argList -Wait -PassThru
    if ($p.ExitCode) { throw "The elevated step failed ($($p.ExitCode))" }
    Write-Host ($(if ($Uninstall) { 'Tether Camera removed.' } else { "Tether Camera installed to $dll" }))
    return
}

# --- elevated from here ---
try {
    if (Test-Path $dll) {
        & regsvr32.exe /s /u $dll
        # The camera service may still hold the old DLL; restarting it frees it.
        Restart-Service FrameServer -Force -ErrorAction SilentlyContinue
        Restart-Service FrameServerMonitor -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
        Remove-Item $dll -Force
    }
    if ($Uninstall) { exit 0 }
    New-Item -ItemType Directory $dest -Force | Out-Null
    Copy-Item $built $dll -Force
    & regsvr32.exe /s $dll
    if ($LASTEXITCODE) { throw "regsvr32 failed ($LASTEXITCODE)" }
    exit 0
} catch {
    Write-Host $_ -ForegroundColor Red
    Start-Sleep 5
    exit 1
}
