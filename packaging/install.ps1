<#
.SYNOPSIS
    Per-user installer for LangCheck (no administrator privileges).

.DESCRIPTION
    Installs langcheck.exe to %LOCALAPPDATA%\Programs\LangCheck, optionally enables
    start-at-login, and optionally launches it in the background. Everything is
    per-user: no service, no elevation, no machine-wide changes (blueprint.md
    Sections 13.4, 21.1).

    NOTE (Step 12 scaffolding): for a real release the executable must be code
    signed first (see packaging/README.md). This script is unsigned scaffolding.

.PARAMETER SourceExe
    Path to the release langcheck.exe. Defaults to one next to this script, then
    ..\target\release\langcheck.exe.

.PARAMETER StartAtLogin
    Register LangCheck to start at sign-in (HKCU Run, via langcheck --register-startup).

.PARAMETER Launch
    Start LangCheck in the background after installing.

.PARAMETER RegisterTsf
    Register the optional TSF precision adapter (corrections in rich/web editors).
    Opt-in and MACHINE-WIDE: it triggers a UAC prompt, unlike the rest of this
    per-user install (TSF text services register under HKLM, like any IME — see
    ADR-0008). The adapter DLL is always copied alongside the exe; this switch also
    registers it. Omit it for a fully per-user, no-elevation install.

.EXAMPLE
    .\install.ps1 -StartAtLogin -Launch

.EXAMPLE
    .\install.ps1 -StartAtLogin -Launch -RegisterTsf   # also enable rich-editor corrections (UAC)
#>
[CmdletBinding()]
param(
    [string]$SourceExe,
    [switch]$StartAtLogin,
    [switch]$Launch,
    [switch]$RegisterTsf
)

$ErrorActionPreference = 'Stop'
$AppName = 'LangCheck'
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

# Locate the executable to install.
if (-not $SourceExe) {
    $candidates = @(
        (Join-Path $ScriptDir 'langcheck.exe'),
        (Join-Path $ScriptDir '..\target\release\langcheck.exe')
    )
    $SourceExe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $SourceExe -or -not (Test-Path $SourceExe)) {
    throw "Could not find langcheck.exe. Pass -SourceExe <path> (build it with 'cargo build --release')."
}

Write-Host "Installing $AppName to $InstallDir ..."
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path $SourceExe -Destination (Join-Path $InstallDir 'langcheck.exe') -Force

# Ship the TSF adapter DLL alongside the exe so it can be registered now or later
# (langcheck --register-tsf resolves it next to langcheck.exe). It is inert until
# registered AND the LangCheck input method is selected.
$sourceDll = Join-Path (Split-Path -Parent $SourceExe) 'langcheck_tsf.dll'
if (Test-Path $sourceDll) {
    Copy-Item -Path $sourceDll -Destination (Join-Path $InstallDir 'langcheck_tsf.dll') -Force
} elseif ($RegisterTsf) {
    Write-Warning "langcheck_tsf.dll not found next to the exe; the TSF adapter cannot be registered."
}

$license = Join-Path $ScriptDir '..\LICENSE'
if (Test-Path $license) {
    Copy-Item -Path $license -Destination (Join-Path $InstallDir 'LICENSE') -Force
}

$exe = Join-Path $InstallDir 'langcheck.exe'

if ($StartAtLogin) {
    Write-Host "Enabling start-at-login ..."
    & $exe --register-startup
}

if ($RegisterTsf -and (Test-Path (Join-Path $InstallDir 'langcheck_tsf.dll'))) {
    Write-Host "Registering the TSF precision adapter (machine-wide; accept the UAC prompt) ..."
    & $exe --register-tsf
}

if ($Launch) {
    Write-Host "Launching $AppName in the background ..."
    Start-Process -FilePath $exe -ArgumentList '--background'
}

Write-Host ""
Write-Host "$AppName installed: $exe"
Write-Host "  Run in background : `"$exe`" --background"
Write-Host "  Start at login    : `"$exe`" --register-startup   (undo: --unregister-startup)"
Write-Host "  TSF adapter       : `"$exe`" --register-tsf   (rich/web editors; UAC; undo: --unregister-tsf)"
Write-Host "  Uninstall         : packaging\uninstall.ps1   (also unregisters the TSF adapter)"
