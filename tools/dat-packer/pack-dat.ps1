param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot "dat-manifest.toml"),
    [string]$OutputPath = (Join-Path (Join-Path $PSScriptRoot "data") "assets.dat"),
    [string]$BaseDir = $PSScriptRoot,
    [UInt64]$ChunkSize = 0,
    [UInt64]$MaxFileSize = 0
)

$ErrorActionPreference = "Stop"

$Packer = Join-Path $PSScriptRoot "dat-packer.exe"
if (-not (Test-Path $Packer)) {
    throw "Missing DAT packer executable: $Packer"
}
if (-not (Test-Path $ManifestPath)) {
    throw "Missing DAT manifest: $ManifestPath"
}
if (-not (Test-Path $BaseDir)) {
    throw "Missing base directory: $BaseDir"
}

$OutputDir = Split-Path -Parent $OutputPath
if (-not [string]::IsNullOrWhiteSpace($OutputDir)) {
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
}

$PackArgs = @(
    "--pack",
    "--input", $ManifestPath,
    "--output", $OutputPath,
    "--base-dir", $BaseDir
)

if ($ChunkSize -gt 0) {
    $PackArgs += @("--chunk-size", $ChunkSize.ToString())
}
if ($MaxFileSize -gt 0) {
    $PackArgs += @("--max-file-size", $MaxFileSize.ToString())
}

& $Packer @PackArgs
if ($LASTEXITCODE -ne 0) {
    throw "DAT packing failed with exit code $LASTEXITCODE"
}

Write-Host "DAT files written from $ManifestPath to $OutputPath"
