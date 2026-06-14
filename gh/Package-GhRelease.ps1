param(
    [string]$OutputDirectory = "artifacts",
    [string]$OperatingSystem = "Windows",
    [string]$Architecture = "x64",
    [string]$Timestamp = (Get-Date -AsUTC -Format 'yyMMddHHmmss'),
    [string]$ArchiveName = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$DropRoot = Join-Path $RepoRoot "drops/flutzplayer"
$OutputRoot = Join-Path $RepoRoot $OutputDirectory

if (-not (Test-Path -LiteralPath $DropRoot)) {
    throw "Drop directory does not exist: $DropRoot"
}

$DropEntries = @(Get-ChildItem -LiteralPath $DropRoot -Force)
if ($DropEntries.Count -eq 0) {
    throw "Drop directory is empty: $DropRoot"
}

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null

if ([string]::IsNullOrWhiteSpace($ArchiveName)) {
    $ArchiveName = "flutzPlayer-$OperatingSystem-$Architecture-$Timestamp.zip"
}

$ArchivePath = Join-Path $OutputRoot $ArchiveName
if (Test-Path -LiteralPath $ArchivePath) {
    Remove-Item -LiteralPath $ArchivePath -Force
}

Compress-Archive -Path (Join-Path $DropRoot "*") -DestinationPath $ArchivePath -CompressionLevel Optimal
Write-Host "Release archive created: $ArchivePath"
