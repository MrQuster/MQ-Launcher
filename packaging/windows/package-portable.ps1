<#
.SYNOPSIS
    Builds the standalone portable ZIP package for MQ Launcher on Windows.
.DESCRIPTION
    Packages mq.exe, portable.flag, mq.ico, run.bat, README.txt, THIRD_PARTY_LICENSES.md,
    and an empty data/ folder flat at the ZIP root into dist/mq-windows-x86_64-portable.zip.
.PARAMETER BinaryPath
    Optional explicit path to mq.exe. If omitted, checks target/release paths.
#>
[CmdletBinding()]
param(
    [string]$BinaryPath = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $scriptDir "..\..")).Path
$distDir = Join-Path $repoRoot "dist"
$packageName = "mq-windows-x86_64-portable"
$archivePath = Join-Path $distDir "$packageName.zip"

Write-Host "=== Packaging MQ Windows Portable Archive ===" -ForegroundColor Cyan

if (-not $BinaryPath) {
    $candidates = @(
        (Join-Path $repoRoot "target\x86_64-pc-windows-msvc\release\mq.exe"),
        (Join-Path $repoRoot "target\release\mq.exe")
    )
    foreach ($cand in $candidates) {
        if (Test-Path -LiteralPath $cand) {
            $BinaryPath = $cand
            break
        }
    }
}

if (-not $BinaryPath -or -not (Test-Path -LiteralPath $BinaryPath)) {
    throw "mq.exe not found! Please build first: cargo build --release`nOr provide -BinaryPath"
}

Write-Host "Source binary: $BinaryPath" -ForegroundColor Gray

if (-not (Test-Path -LiteralPath $distDir)) {
    New-Item -ItemType Directory -Path $distDir -Force | Out-Null
}

$tempStage = Join-Path ([System.IO.Path]::GetTempPath()) ("mq-portable-" + [System.Guid]::NewGuid().ToString("N"))

try {
    New-Item -ItemType Directory -Path $tempStage -Force | Out-Null

    Copy-Item -LiteralPath $BinaryPath -Destination (Join-Path $tempStage "mq.exe") -Force
    Copy-Item -LiteralPath (Join-Path $scriptDir "portable.flag") -Destination (Join-Path $tempStage "portable.flag") -Force
    Copy-Item -LiteralPath (Join-Path $scriptDir "mq.ico") -Destination (Join-Path $tempStage "mq.ico") -Force
    Copy-Item -LiteralPath (Join-Path $scriptDir "run.bat") -Destination (Join-Path $tempStage "run.bat") -Force
    Copy-Item -LiteralPath (Join-Path $scriptDir "README-portable.txt") -Destination (Join-Path $tempStage "README.txt") -Force

    $thirdParty = Join-Path $repoRoot "THIRD_PARTY_LICENSES.md"
    if (Test-Path -LiteralPath $thirdParty) {
        Copy-Item -LiteralPath $thirdParty -Destination (Join-Path $tempStage "THIRD_PARTY_LICENSES.md") -Force
    }
    $licenseFile = Join-Path $repoRoot "LICENSE"
    if (Test-Path -LiteralPath $licenseFile) {
        Copy-Item -LiteralPath $licenseFile -Destination (Join-Path $tempStage "LICENSE") -Force
    }
    New-Item -ItemType Directory -Path (Join-Path $tempStage "data") -Force | Out-Null

    if (Test-Path -LiteralPath $archivePath) {
        Remove-Item -LiteralPath $archivePath -Force
    }

    Write-Host "Compressing to $archivePath..." -ForegroundColor Cyan
    Compress-Archive -Path "$tempStage\*" -DestinationPath $archivePath -CompressionLevel Optimal -Force

    $hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "[OK] Archive created: $archivePath" -ForegroundColor Green
    Write-Host "SHA-256: $hash" -ForegroundColor Yellow
}
finally {
    if (Test-Path -LiteralPath $tempStage) {
        Remove-Item -LiteralPath $tempStage -Recurse -Force -ErrorAction SilentlyContinue
    }
}
