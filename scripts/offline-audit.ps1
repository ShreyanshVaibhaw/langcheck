<#
.SYNOPSIS
    Offline-invariant audit: fail if the source introduces networking or telemetry.

.DESCRIPTION
    LangCheck is fully local — no networking, telemetry, crash upload, remote
    logging, cloud sync, or update client (blueprint.md Sections 1.1, 16; the
    non-negotiable privacy invariant). `cargo deny` bans known networking/telemetry
    *crates*; this audit is the complementary source-level guard: it fails if any
    workspace source uses raw networking primitives (which need no extra crate), or
    if any manifest enables a networking/HTTP Windows feature.

    Runs in CI (via pwsh on the Linux runner) and locally:  pwsh scripts\offline-audit.ps1
    Exit code 0 = clean; 1 = violations found.
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

# Raw networking primitives in Rust source (no crate needed to reach the network).
$forbiddenSource = @(
    'std::net',
    '\bTcpStream\b',
    '\bTcpListener\b',
    '\bUdpSocket\b',
    '\bto_socket_addrs\b',
    '\bToSocketAddrs\b'
)
# Networking / HTTP Windows feature areas that must never be enabled.
$forbiddenFeatures = @(
    'Win32_Networking',
    'Win32_NetworkManagement',
    'Win32_Web',
    'WinHttp',
    'WinINet'
)

$violations = [System.Collections.Generic.List[string]]::new()

$srcRoots = @((Join-Path $root 'crates'), (Join-Path $root 'tools')) | Where-Object { Test-Path $_ }
$rsFiles = Get-ChildItem -Path $srcRoots -Recurse -Filter *.rs -ErrorAction SilentlyContinue
foreach ($file in $rsFiles) {
    $text = Get-Content -Raw -LiteralPath $file.FullName
    foreach ($pat in $forbiddenSource) {
        if ($text -match $pat) {
            $violations.Add("$($file.FullName): networking primitive /$pat/")
        }
    }
}

$tomls = Get-ChildItem -Path $root -Recurse -Filter Cargo.toml -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' }
foreach ($toml in $tomls) {
    $text = Get-Content -Raw -LiteralPath $toml.FullName
    foreach ($feat in $forbiddenFeatures) {
        if ($text -match $feat) {
            $violations.Add("$($toml.FullName): forbidden feature '$feat'")
        }
    }
}

if ($violations.Count -gt 0) {
    Write-Host "OFFLINE AUDIT FAILED — networking/telemetry surface introduced:"
    $violations | ForEach-Object { Write-Host "  $_" }
    exit 1
}

Write-Host "Offline audit passed: scanned $($rsFiles.Count) source files + $($tomls.Count) manifests; no networking/telemetry primitives or features found."
