param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Release"
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$SoundfontBootstrap = Join-Path $RepoRoot "soundfonts/get-fonts.ps1"
$ManifestPath = Join-Path $RepoRoot "assets/dat-manifest.toml"
$GeneratedDatRoot = Join-Path $RepoRoot "_local/generated-assets/dat"
$GeneratedDatFile = Join-Path $GeneratedDatRoot "assets.dat"

if (-not (Test-Path $SoundfontBootstrap)) {
    throw "Missing soundfont bootstrap helper: $SoundfontBootstrap"
}

& $SoundfontBootstrap

if (-not (Test-Path $ManifestPath)) {
    throw "Missing DAT manifest: $ManifestPath"
}

New-Item -ItemType Directory -Force -Path $GeneratedDatRoot | Out-Null

$PackArgs = @("run", "-p", "flutz_soundfont_tools")
if ($Configuration -eq "Release") {
    $PackArgs += "--release"
}
$PackArgs += @(
    "--",
    "--pack",
    "--input", $ManifestPath,
    "--output", $GeneratedDatFile,
    "--base-dir", $RepoRoot
)

& cargo @PackArgs
if ($LASTEXITCODE -ne 0) {
    throw "DAT packing failed with exit code $LASTEXITCODE"
}

$datFiles = @(Get-ChildItem -Path $GeneratedDatRoot -Filter "*.dat" -File -ErrorAction SilentlyContinue)
if ($datFiles.Count -eq 0) {
    throw "DAT packing completed but no DAT files were found in $GeneratedDatRoot"
}

Write-Host "Generated DAT assets:"
foreach ($datFile in $datFiles) {
    Write-Host " - $($datFile.FullName)"
}
