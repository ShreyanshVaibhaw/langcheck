<#
.SYNOPSIS
    Per-user uninstaller for LangCheck.

.DESCRIPTION
    Removes start-at-login, deletes the installed executable, and (only if asked)
    deletes user state. Per-user only; no elevation. Uninstall always removes the
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
