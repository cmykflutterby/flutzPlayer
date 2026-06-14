param(
    [string]$OutputDirectory = "artifacts",
    [string]$ArchiveName = "flutzplayer-windows-x64.zip"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$DropRoot = Join-Path $RepoRoot "drops/flutzplayer"
$OutputRoot = Join-Path $RepoRoot $OutputDirectory
$ArchivePath = Join-Path $OutputRoot $ArchiveName

if (-not (Test-Path -LiteralPath $DropRoot)) {
    throw "Drop directory does not exist: $DropRoot"
}

$DropEntries = @(Get-ChildItem -LiteralPath $DropRoot -Force)
if ($DropEntries.Count -eq 0) {
    throw "Drop directory is empty: $DropRoot"
}

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null

if (Test-Path -LiteralPath $ArchivePath) {
    Remove-Item -LiteralPath $ArchivePath -Force
}

Compress-Archive -Path (Join-Path $DropRoot "*") -DestinationPath $ArchivePath -CompressionLevel Optimal
Write-Host "Release archive created: $ArchivePath"
