<#
.SYNOPSIS
    Per-user uninstaller for LangCheck.

.DESCRIPTION
    Removes start-at-login, unregisters the TSF adapter if it was registered,
    deletes the installed executable, and (only if asked) deletes user state.
    Per-user, with one exception: if the optional TSF adapter was registered, its
    removal is machine-wide and prompts for elevation (UAC) — it must be undone or a
    broken input method would point at the deleted DLL. Uninstall always removes the
    startup registration (blueprint.md Sections 21.1, 13.4).

.PARAMETER DeleteState
    Also delete user state under %LOCALAPPDATA%\LangCheck (config, personal
    dictionary). Off by default — user state is retained unless requested.

.EXAMPLE
    .\uninstall.ps1            # remove app + startup; keep settings
    .\uninstall.ps1 -DeleteState
#>
[CmdletBinding()]
param(
    [switch]$DeleteState
)

$ErrorActionPreference = 'Stop'
$AppName = 'LangCheck'
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$exe = Join-Path $InstallDir 'langcheck.exe'

if (Test-Path $exe) {
    Write-Host "Removing start-at-login ..."
    try { & $exe --unregister-startup } catch { Write-Warning "could not unregister startup: $_" }

    if ($DeleteState) {
        Write-Host "Deleting user state ..."
        try { & $exe --reset } catch { Write-Warning "could not delete state via app: $_" }
    }
}

# Unregister the TSF adapter if it was registered (machine-wide CLSID under HKLM).
# Must happen BEFORE the DLL is deleted, and we wait for the elevated step to finish
# so DllUnregisterServer can still load the DLL. Only prompt for UAC if it is
# actually registered (reading HKLM needs no elevation).
$tsfClsidKey = 'HKLM:\Software\Classes\CLSID\{4C434B54-5346-4D56-5001-000000000001}'
if ((Test-Path $exe) -and (Test-Path $tsfClsidKey)) {
    Write-Host "Unregistering the TSF adapter (machine-wide; accept the UAC prompt) ..."
    try {
        Start-Process -FilePath $exe -ArgumentList '--unregister-tsf' -Verb RunAs -Wait
    } catch {
        Write-Warning "could not unregister the TSF adapter; run '$exe --unregister-tsf' manually before deleting files: $_"
    }
}

Write-Host "Deleting $InstallDir ..."
if (Test-Path $InstallDir) {
    Remove-Item -Path $InstallDir -Recurse -Force
}

# Belt-and-suspenders: remove the state directory directly if requested.
if ($DeleteState) {
    $stateDir = Join-Path $env:LOCALAPPDATA $AppName
    if (Test-Path $stateDir) {
        Remove-Item -Path $stateDir -Recurse -Force
    }
}

Write-Host "$AppName uninstalled."
if (-not $DeleteState) {
    Write-Host "User settings were kept (re-run with -DeleteState to remove them)."
}
