param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Release",
    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$DatManifest = Join-Path $RepoRoot "assets/dat-manifest.toml"
$SoundfontDir = Join-Path $RepoRoot "soundfonts"
$GeneratedDatRoot = Join-Path $RepoRoot "_local/generated-assets/dat"
$CacheDir = Join-Path $RepoRoot ".gh-cache/dat-packs"

function Get-FileHashHex {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return ""
    }

    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Get-SoundfontSetHash {
    param([Parameter(Mandatory = $true)][string]$Directory)

    if (-not (Test-Path -LiteralPath $Directory)) {
        return ""
    }

    $files = @(Get-ChildItem -LiteralPath $Directory -Filter "*.sf2" -File | Sort-Object -Property Name)
    if ($files.Count -eq 0) {
        return ""
    }

    $parts = foreach ($file in $files) {
        "$($file.Name):$(Get-FileHashHex -Path $file.FullName)"
    }

    $joined = [string]::Join("|", $parts)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($joined)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $digest = $sha.ComputeHash($bytes)
    }
    finally {
        $sha.Dispose()
    }

    return ([System.BitConverter]::ToString($digest)).Replace("-", "").ToLowerInvariant()
}

function Resolve-CargoExecutable {
    $cargoCommand = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $cargoCommand) {
        throw "Could not locate cargo in the active environment."
    }

    $cargoDirectory = Split-Path -Parent $cargoCommand.Source
    if ($env:PATH -notlike "*$cargoDirectory*") {
        $env:PATH = "$cargoDirectory;$env:PATH"
    }

    return $cargoCommand.Source
}

function Invoke-Cargo {
    param(
        [Parameter(Mandatory = $true)][string]$CargoExe,
        [Parameter(Mandatory = $true)][string[]]$Args,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory
    )

    Push-Location $WorkingDirectory
    try {
        & $CargoExe @Args
        if ($LASTEXITCODE -ne 0) {
            throw "$CargoExe $($Args -join ' ') failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $DatManifest)) {
    throw "DAT manifest not found: $DatManifest"
}

if (-not (Test-Path -LiteralPath $SoundfontDir)) {
    Write-Host "Soundfont directory missing; skipping DAT generation for this run"
    return
}

$soundfontFiles = @(Get-ChildItem -LiteralPath $SoundfontDir -Filter "*.sf2" -File)
if ($soundfontFiles.Count -eq 0) {
    Write-Host "No soundfont files present; skipping DAT generation for this run"
    return
}

Write-Host "Found $($soundfontFiles.Count) soundfont file(s)"

$manifestHash = Get-FileHashHex -Path $DatManifest
$soundfontHash = Get-SoundfontSetHash -Directory $SoundfontDir
$cacheKey = "$manifestHash-$soundfontHash"
$cacheMarkerPath = Join-Path $CacheDir ".cache-key"

$useCache = $false
if ((Test-Path -LiteralPath $cacheMarkerPath) -and (Test-Path -LiteralPath $CacheDir)) {
    $cachedKey = (Get-Content -LiteralPath $cacheMarkerPath -Raw).Trim()
    if ($cachedKey -eq $cacheKey) {
        $cachedDat = @(Get-ChildItem -LiteralPath $CacheDir -Filter "*.dat" -File -ErrorAction SilentlyContinue)
        if ($cachedDat.Count -gt 0) {
            $useCache = $true
            Write-Host "Using cached DAT artifacts ($($cachedDat.Count) file(s))"
        }
    }
}

if (-not $useCache) {
    Write-Host "DAT cache miss; generating DAT artifacts"

    if (Test-Path -LiteralPath $GeneratedDatRoot) {
        Remove-Item -Recurse -Force -LiteralPath $GeneratedDatRoot
    }
    New-Item -ItemType Directory -Force -Path $GeneratedDatRoot | Out-Null

    $cargoExe = Resolve-CargoExecutable
    $cargoModeArgs = if ($Configuration -eq "Release") { @("--release") } else { @() }
    $datOutput = Join-Path $GeneratedDatRoot "assets.dat"

    $packArgs = @("run", "-p", "flutz_soundfont_tools") + $cargoModeArgs + @(
        "--",
        "--pack",
        "--input", $DatManifest,
        "--output", $datOutput,
        "--base-dir", $RepoRoot
    )

    Invoke-Cargo -CargoExe $cargoExe -Args $packArgs -WorkingDirectory $RepoRoot

    $generatedDat = @(Get-ChildItem -LiteralPath $GeneratedDatRoot -Filter "*.dat" -File -ErrorAction SilentlyContinue)
    if ($generatedDat.Count -eq 0) {
        throw "No DAT files were generated in $GeneratedDatRoot"
    }

    New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null
    Get-ChildItem -LiteralPath $CacheDir -Filter "*.dat" -File -ErrorAction SilentlyContinue | Remove-Item -Force

    foreach ($datFile in $generatedDat) {
        Copy-Item -Force -LiteralPath $datFile.FullName -Destination (Join-Path $CacheDir $datFile.Name)
    }

    Set-Content -LiteralPath $cacheMarkerPath -Value $cacheKey -NoNewline
    Write-Host "Generated and cached $($generatedDat.Count) DAT file(s)"
}

$cachedDatFiles = @(Get-ChildItem -LiteralPath $CacheDir -Filter "*.dat" -File -ErrorAction SilentlyContinue)
if ($cachedDatFiles.Count -eq 0) {
    throw "DAT cache is empty after generation attempt: $CacheDir"
}

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
foreach ($datFile in $cachedDatFiles) {
    Copy-Item -Force -LiteralPath $datFile.FullName -Destination (Join-Path $OutputDirectory $datFile.Name)
}

Write-Host "Staged $($cachedDatFiles.Count) DAT file(s) into $OutputDirectory"
