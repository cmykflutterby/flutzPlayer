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
$TargetRoot = Join-Path $RepoRoot "target"
$DatPackerWrapper = Join-Path $RepoRoot "tools/dat-packer/pack-dat.ps1"

function Get-FileHash {
    param([Parameter(Mandatory = $true)][string]$FilePath)
    
    if (-not (Test-Path -LiteralPath $FilePath)) {
        return ""
    }
    
    $hash = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($FilePath)
        $bytes = $hash.ComputeHash($stream)
        return [System.BitConverter]::ToString($bytes).Replace("-", "").ToLower()
    } finally {
        $stream.Dispose()
    }
}

function Get-DirectoryHash {
    param([Parameter(Mandatory = $true)][string]$DirectoryPath)
    
    if (-not (Test-Path -LiteralPath $DirectoryPath)) {
        return ""
    }
    
    $files = @(Get-ChildItem -LiteralPath $DirectoryPath -Filter "*.sf2" -File | Sort-Object FullName)
    if ($files.Count -eq 0) {
        return ""
    }
    
    $hashInput = $files | ForEach-Object { (Get-FileHash -FilePath $_.FullName) } | Join-String
    $hash = [System.Security.Cryptography.SHA256]::Create()
    $bytes = $hash.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($hashInput))
    return [System.BitConverter]::ToString($bytes).Replace("-", "").ToLower()
}

function Resolve-CargoExecutable {
    $cargoCommand = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $cargoCommand) {
        throw 'Could not locate cargo in the active environment.'
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
        [Parameter(Mandatory = $true)][string[]]$Args
    )

    & $CargoExe @Args
    if ($LASTEXITCODE -ne 0) {
        throw "$CargoExe $($Args -join ' ') failed with exit code $LASTEXITCODE"
    }
}

# Verify manifest and soundfonts exist
if (-not (Test-Path -LiteralPath $DatManifest)) {
    throw "DAT manifest not found: $DatManifest"
}
if (-not (Test-Path -LiteralPath $SoundfontDir)) {
    throw "Soundfont directory not found: $SoundfontDir"
}

$soundfontFiles = @(Get-ChildItem -LiteralPath $SoundfontDir -Filter "*.sf2" -File)
if ($soundfontFiles.Count -eq 0) {
    throw "No soundfont files found in $SoundfontDir"
}

Write-Host "Found $($soundfontFiles.Count) soundfont files"

# Calculate cache key based on manifest and soundfonts
$manifestHash = Get-FileHash -FilePath $DatManifest
$soundfontHash = Get-DirectoryHash -DirectoryPath $SoundfontDir
$cacheKey = "$manifestHash-$soundfontHash"
$cacheMarkerFile = Join-Path $CacheDir ".cache-key"

# Check if cached DAT files are still valid
$useCached = $false
$cacheMessage = ""

if ((Test-Path -LiteralPath $cacheDir) -and (Test-Path -LiteralPath $cacheMarkerFile)) {
    $cachedKey = Get-Content -LiteralPath $cacheMarkerFile -Raw
    if ($cachedKey -eq $cacheKey) {
        $cachedDatFiles = @(Get-ChildItem -LiteralPath $cacheDir -Filter "*.dat" -File)
        if ($cachedDatFiles.Count -gt 0) {
            $useCached = $true
            $cacheMessage = "Using $($cachedDatFiles.Count) cached DAT file(s) (manifest and soundfonts unchanged)"
        }
    }
}

if ($useCached) {
    Write-Host $cacheMessage
} else {
    Write-Host "Cache miss or invalid; regenerating DAT files"
    
    # Clear and recreate working directories
    if (Test-Path -LiteralPath $GeneratedDatRoot) {
        Remove-Item -Recurse -Force -LiteralPath $GeneratedDatRoot
    }
    New-Item -ItemType Directory -Force -Path $GeneratedDatRoot | Out-Null
    
    # Pack DAT files
    Write-Host "Packing DAT assets"
    $cargoExe = Resolve-CargoExecutable
    
    $profileFolder = if ($Configuration -eq "Release") { "release" } else { "debug" }
    $cargoModeArgs = if ($Configuration -eq "Release") { @("--release") } else { @() }
    
    $datPackArgs = @("run", "-p", "flutz_soundfont_tools") + $cargoModeArgs + @(
        "--",
        "--pack",
        "--input", $DatManifest,
        "--output", (Join-Path $GeneratedDatRoot "assets.dat"),
        "--base-dir", $RepoRoot
    )
    
    Push-Location $RepoRoot
    try {
        Invoke-Cargo -CargoExe $cargoExe -Args $datPackArgs
    } finally {
        Pop-Location
    }
    
    # Verify DAT files were generated
    $generatedDatFiles = @(Get-ChildItem -LiteralPath $GeneratedDatRoot -Filter "*.dat" -File)
    if ($generatedDatFiles.Count -eq 0) {
        throw "No DAT files generated in $GeneratedDatRoot"
    }
    
    # Cache the generated DAT files
    New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null
    foreach ($datFile in $generatedDatFiles) {
        Copy-Item -Force -LiteralPath $datFile.FullName -Destination (Join-Path $CacheDir $datFile.Name)
    }
    $cacheKey | Out-File -LiteralPath $cacheMarkerFile -Encoding utf8 -NoNewline
    Write-Host "Cached $($generatedDatFiles.Count) DAT file(s) with key: $($cacheKey.Substring(0, 16))..."
}

# Copy DAT files from cache to output
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$cachedDatFiles = @(Get-ChildItem -LiteralPath $CacheDir -Filter "*.dat" -File)
if ($cachedDatFiles.Count -eq 0) {
    throw "No DAT files available in cache: $CacheDir"
}
foreach ($datFile in $cachedDatFiles) {
    Copy-Item -Force -LiteralPath $datFile.FullName -Destination (Join-Path $OutputDirectory $datFile.Name)
}

Write-Host "Staged $($cachedDatFiles.Count) DAT file(s) for release"
