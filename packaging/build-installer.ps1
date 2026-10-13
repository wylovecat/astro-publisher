# Build the Windows installer for astro-publisher.
#
# The point of this script is that the version has exactly one source:
# `Cargo.toml` (via `cargo metadata`, which resolves workspace inheritance).
# Building the installer used to mean hand-typing `/DAPP_VERSION=0.1.0` next to
# a `Cargo.toml` that said something else, and the two drifting apart silently
# — the installed Add/Remove entry, the exe metadata and the file name would
# all claim a version the crate never had.
#
# NOTE: this file is deliberately pure ASCII. A .ps1 containing non-ASCII is
# read with whatever the machine's ANSI codepage happens to be, and a mangled
# byte inside a string literal breaks the parse ("'C' is not recognized as a
# cmdlet"). Keeping it ASCII makes the script encoding-independent.
#
# Usage:
#   pwsh packaging\build-installer.ps1                 # build + package
#   pwsh packaging\build-installer.ps1 -SkipBuild      # reuse target\release
#   pwsh packaging\build-installer.ps1 -Sign           # sign if a cert is set up
#
# Code signing is opt-in and needs a certificate you own; see the README.
[CmdletBinding()]
param(
    # Reuse whatever is already in target\release instead of rebuilding.
    [switch]$SkipBuild,

    # Package even when target\release is older than the source tree
    # (otherwise that is refused, because it silently ships stale code).
    [switch]$Force,

    # Sign the installer after packaging. Requires -CertificateThumbprint or
    # -CertificatePath; without either, the script explains and stops.
    [switch]$Sign,

    # Thumbprint of a certificate in the user's store (signtool /sha1).
    [string]$CertificateThumbprint,

    # Path to a .pfx file (signtool /f), with -CertificatePassword if needed.
    [string]$CertificatePath,
    [string]$CertificatePassword,

    # RFC 3161 timestamp server. Defaults to DigiCert's public one.
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    # --- Version: Cargo.toml is the only source -----------------------------
    $metadata = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq 'astro-publisher' }
    if (-not $package) { throw 'cargo metadata did not report an astro-publisher package' }
    $version = $package.version
    Write-Host "version: $version (from Cargo.toml)"

    # NSIS writes VIProductVersion as four numbers; a three-part version needs
    # the trailing field filled in or makensis rejects the script.
    $versionQuad = if (($version -split '\.').Count -ge 4) { $version } else { "$version.0" }

    # --- Fail fast on a signing request we cannot satisfy -------------------
    # Checked before the build: there is no point compiling for two minutes to
    # then discover the certificate was never configured.
    if ($Sign) {
        if (-not (Get-Command signtool -ErrorAction SilentlyContinue)) {
            throw 'signtool not found. It ships with the Windows SDK (Signing Tools).'
        }
        if (-not $CertificateThumbprint -and -not $CertificatePath) {
            throw @'
-Sign needs a certificate: pass -CertificateThumbprint (store) or
-CertificatePath (a .pfx).

A code-signing certificate is issued to you (or your organisation) by a CA
after identity verification: it cannot be generated locally, and this script
will not pretend otherwise. Without one the installer still builds and works;
Windows just shows the SmartScreen prompt on first run. See the installer
section of README.md.
'@
        }
    }

    # --- Binaries -----------------------------------------------------------
    if (-not $SkipBuild) {
        Write-Host 'building release binaries...'
        cargo build --release -p astro-publisher -p astro-publisher-core
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    }

    $sourceDir = Join-Path $root 'target\release'
    foreach ($exe in 'astro-publisher.exe', 'astro-publisher-core.exe') {
        if (-not (Test-Path (Join-Path $sourceDir $exe))) {
            throw "$exe not found in $sourceDir - run without -SkipBuild first"
        }
    }

    # --- Refuse to package binaries older than the source -------------------
    # `-SkipBuild` once shipped an installer whose app predated a UI fix,
    # because target\release still held a months-old binary while every source
    # file was newer. Packaging that binary looks successful and produces a
    # broken installer, so compare the newest source mtime against the binary
    # and stop unless the caller insists.
    $appExe = Join-Path $sourceDir 'astro-publisher.exe'
    $binaryTime = (Get-Item $appExe).LastWriteTimeUtc

    $newestSource = Get-ChildItem -Path (Join-Path $root 'crates'), (Join-Path $root 'Cargo.toml') `
            -Recurse -File -Include '*.rs', 'Cargo.toml' -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notmatch '\\target\\' } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1

    if ($newestSource -and $newestSource.LastWriteTimeUtc -gt $binaryTime) {
        $msg = @"
target\release\astro-publisher.exe is older than the source tree:
  binary: $binaryTime
  newest: $($newestSource.FullName)  ($($newestSource.LastWriteTimeUtc))

Packaging it would ship an installer built from stale code. Re-run without
-SkipBuild to rebuild first.
"@
        if (-not $Force) { throw $msg }
        Write-Warning $msg
    }

    $distDir = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force -Path $distDir | Out-Null
    $outFile = Join-Path $distDir "astro-publisher-$version-setup.exe"

    # --- Package ------------------------------------------------------------
    # NSIS does not put itself on PATH, so fall back to its install location.
    $makensis = Get-Command makensis -ErrorAction SilentlyContinue
    if ($makensis) {
        $makensis = $makensis.Source
    } else {
        # @() matters: with a single surviving path the pipeline collapses to a
        # bare string, and $candidates[0] would then be its first *character*.
        $candidates = @(
            @(
                (Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe')
                (Join-Path $env:ProgramFiles 'NSIS\makensis.exe')
                (Join-Path $env:LOCALAPPDATA 'Programs\NSIS\makensis.exe')
            ) | Where-Object { $_ -and (Test-Path $_) }
        )

        if ($candidates.Count -eq 0) {
            throw 'makensis not found. Install NSIS: winget install NSIS.NSIS'
        }
        $makensis = $candidates[0]
    }
    Write-Host "makensis: $makensis"

    Write-Host "packaging $($outFile)..."
    # An explicit argument array: NSIS switches contain `=`, `\` and spaces, and
    # an array keeps each one a single argument without quoting games.
    $nsisArgs = @(
        "/DAPP_VERSION=$versionQuad"
        "/DSOURCE_DIR=$sourceDir"
        "/DOUT_FILE=$outFile"
        (Join-Path $root 'packaging\installer.nsi')
    )
    & $makensis @nsisArgs
    if ($LASTEXITCODE -ne 0) { throw "makensis failed ($LASTEXITCODE)" }

    # --- Sign (opt-in; needs a certificate you own) -------------------------
    if ($Sign) {
        # Validated up front, so this only runs with a usable certificate.
        $signtool = (Get-Command signtool).Source
        $signArgs = @('sign', '/fd', 'SHA256', '/tr', $TimestampUrl, '/td', 'SHA256')
        if ($CertificateThumbprint) {
            $signArgs += @('/sha1', $CertificateThumbprint)
        } else {
            $signArgs += @('/f', $CertificatePath)
            if ($CertificatePassword) { $signArgs += @('/p', $CertificatePassword) }
        }

        Write-Host 'signing...'
        & $signtool @signArgs $outFile
        if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE)" }

        # Prove the signature rather than assuming it.
        & $signtool 'verify' '/pa' '/v' $outFile
        if ($LASTEXITCODE -ne 0) { throw 'signature verification failed' }
    } else {
        Write-Host 'not signed (pass -Sign with a certificate to sign)'
    }

    # --- Manifest -----------------------------------------------------------
    # Users check the download against this, so it is produced by the same run
    # that produced the file.
    $hash = (Get-FileHash -Algorithm SHA256 -Path $outFile).Hash
    $name = Split-Path -Leaf $outFile
    Set-Content -Path (Join-Path $distDir 'SHA256SUMS.txt') -Value "$hash  $name" -Encoding UTF8

    Write-Host ''
    Write-Host "installer: $outFile"
    Write-Host "sha256:    $hash"
}
finally {
    Pop-Location
}
