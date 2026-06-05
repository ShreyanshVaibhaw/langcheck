<#
.SYNOPSIS
    Assemble a distributable LangCheck package (.zip) from a release build.

.DESCRIPTION
    Stages langcheck.exe, langcheck_tsf.dll, LICENSE, the install/uninstall scripts,
    a packaging note, and any SBOM into a versioned zip under dist\ (release-process
    step 6; see packaging/README.md). Per-user, no admin, no network.

    Does NOT code-sign. Signing langcheck.exe AND langcheck_tsf.dll is a separate
    manual step that must be done BEFORE packaging for distribution (step 4); this
    script warns if the binaries are unsigned.

.PARAMETER ReleaseDir
    Release build directory (default: ..\target\release).

.PARAMETER OutDir
    Output directory for the zip (default: ..\dist).

.EXAMPLE
    cargo build --workspace --release
    packaging\package.ps1
#>
[CmdletBinding()]
param(
    [string]$ReleaseDir,
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir
if (-not $ReleaseDir) { $ReleaseDir = Join-Path $RepoRoot 'target\release' }
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot 'dist' }

$exe = Join-Path $ReleaseDir 'langcheck.exe'
$dll = Join-Path $ReleaseDir 'langcheck_tsf.dll'
foreach ($required in @($exe, $dll)) {
    if (-not (Test-Path $required)) {
        throw "Missing $required — run 'cargo build --workspace --release' first."
    }
}

# Version from the workspace manifest ([workspace.package] version).
$versionMatch = Select-String -Path (Join-Path $RepoRoot 'Cargo.toml') -Pattern '^\s*version\s*=\s*"([^"]+)"' |
    Select-Object -First 1
$version = if ($versionMatch) { $versionMatch.Matches.Groups[1].Value } else { '0.0.0' }

$stageName = "langcheck-$version-x86_64-pc-windows-msvc"
$stage = Join-Path $OutDir $stageName
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item $exe $stage
Copy-Item $dll $stage
Copy-Item (Join-Path $ScriptDir 'install.ps1') $stage
Copy-Item (Join-Path $ScriptDir 'uninstall.ps1') $stage
$license = Join-Path $RepoRoot 'LICENSE'
if (Test-Path $license) { Copy-Item $license $stage }
$pkgReadme = Join-Path $ScriptDir 'README.md'
if (Test-Path $pkgReadme) { Copy-Item $pkgReadme (Join-Path $stage 'PACKAGING.md') }
# Bundle an SBOM if one was generated next to the repo (CI emits langcheck.cdx.json).
Get-ChildItem -Path $RepoRoot -Filter '*.cdx.json' -File -ErrorAction SilentlyContinue |
    ForEach-Object { Copy-Item $_.FullName $stage }

# Informational: warn if the binaries are not Authenticode-signed.
foreach ($bin in @((Join-Path $stage 'langcheck.exe'), (Join-Path $stage 'langcheck_tsf.dll'))) {
    $sig = Get-AuthenticodeSignature $bin
    if ($sig.Status -ne 'Valid') {
        Write-Warning "$(Split-Path -Leaf $bin) is NOT signed ($($sig.Status)). For DISTRIBUTION, code-sign langcheck.exe + langcheck_tsf.dll BEFORE running this (packaging/README.md step 4)."
    }
}

$zip = Join-Path $OutDir "$stageName.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Remove-Item $stage -Recurse -Force

Write-Host "Packaged: $zip"
Get-ChildItem $zip | Select-Object -Property Name, Length | Format-Table -AutoSize
