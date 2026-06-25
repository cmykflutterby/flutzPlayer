param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Release",

    [switch]$SkipBuild,
    [switch]$RebuildDat,
    [switch]$BuildDat,
    [switch]$CleanDrop,
    [switch]$NoJemallocMemory
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$DropRoot = Join-Path $RepoRoot "drops/flutzplayer"
$DropData = Join-Path $DropRoot "data"
$DropDatPacker = Join-Path $DropRoot "dat-packer"
$VendorSdl3 = Join-Path $RepoRoot "vendor/SDL3"
$DatBuildScript = Join-Path $RepoRoot "build-dat.ps1"
$DatManifest = Join-Path $RepoRoot "assets/dat-manifest.toml"
$DatPackerWrapper = Join-Path $RepoRoot "tools/dat-packer/pack-dat.ps1"
$GeneratedDatRoot = Join-Path $RepoRoot "_local/generated-assets/dat"
$TargetRoot = Join-Path $RepoRoot "_local/target"

if ($CleanDrop -and (Test-Path $DropRoot)) {
    Remove-Item -Recurse -Force $DropRoot
}

if (Test-Path $VendorSdl3) {
    Write-Host "Using vendored SDL3 at $VendorSdl3"
    $env:FLUTZ_SDL3_DIR = $VendorSdl3
} else {
    Write-Host "Vendored SDL3 not present yet. Expected location: $VendorSdl3"
}

if (-not $SkipBuild) {
    $CargoArgs = @("build", "--workspace")
    if ($Configuration -eq "Release") {
        $CargoArgs += "--release"
    }
    if (-not $NoJemallocMemory) {
        $CargoArgs += @("--features", "flutzplayer/jemalloc-memory")
        Write-Host "Building with flutzplayer/jemalloc-memory"
    }

    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo build failed with exit code $LASTEXITCODE"
    }
}

if ($RebuildDat -or $BuildDat) {
    if (-not (Test-Path $DatBuildScript)) {
        throw "Missing DAT build script: $DatBuildScript"
    }

    & $DatBuildScript -Configuration $Configuration
    if ($LASTEXITCODE -ne 0) {
        throw "DAT packing failed with exit code $LASTEXITCODE"
    }
}

New-Item -ItemType Directory -Force -Path $DropRoot | Out-Null
New-Item -ItemType Directory -Force -Path $DropData | Out-Null
New-Item -ItemType Directory -Force -Path $DropDatPacker | Out-Null

Get-ChildItem -Path $DropRoot -Filter "SDL3.dll" -File -Recurse -ErrorAction SilentlyContinue | Remove-Item -Force
Get-ChildItem -Path $DropData -Filter "*.toml" -File -ErrorAction SilentlyContinue | Remove-Item -Force
Get-ChildItem -Path $DropData -Filter "*.dat" -File -ErrorAction SilentlyContinue | Remove-Item -Force

$ProfileFolder = if ($Configuration -eq "Release") { "release" } else { "debug" }
$ExeName = if ($IsWindows -or $env:OS -eq "Windows_NT") { "flutzplayer.exe" } else { "flutzplayer" }
$DropExeName = if ($Configuration -eq "Debug") {
    if ($ExeName.EndsWith(".exe")) { "flutzplayer-debug.exe" } else { "flutzplayer-debug" }
} else {
    $ExeName
}
$BuiltExe = Join-Path $TargetRoot "$ProfileFolder/$ExeName"
$BuiltDatPacker = Join-Path $TargetRoot "$ProfileFolder/flutz_soundfont_tools.exe"

if (Test-Path $BuiltExe) {
    $StagedExe = Join-Path $DropRoot $DropExeName
    $BuiltExeItem = Get-Item $BuiltExe
    $StagedExeItem = if (Test-Path $StagedExe) { Get-Item $StagedExe } else { $null }
    if ($null -ne $StagedExeItem -and $BuiltExeItem.Length -eq $StagedExeItem.Length -and $BuiltExeItem.LastWriteTimeUtc -eq $StagedExeItem.LastWriteTimeUtc) {
        Write-Host "Application binary already staged at $StagedExe"
    } else {
        Copy-Item -Force $BuiltExe $StagedExe
    }
} else {
    Write-Host "Application binary not found yet: $BuiltExe"
}

if (Test-Path $BuiltDatPacker) {
    Copy-Item -Force $BuiltDatPacker (Join-Path $DropDatPacker "dat-packer.exe")
} else {
    Write-Host "DAT packer binary not found yet: $BuiltDatPacker"
}

if (Test-Path $DatPackerWrapper) {
    Copy-Item -Force $DatPackerWrapper (Join-Path $DropDatPacker "pack-dat.ps1")
} else {
    Write-Host "DAT packer wrapper not found yet: $DatPackerWrapper"
}

if (Test-Path $DatManifest) {
    Copy-Item -Force $DatManifest (Join-Path $DropDatPacker "dat-manifest.toml")
} else {
    Write-Host "DAT manifest not found yet: $DatManifest"
}

$GeneratedDatFiles = @()
if (Test-Path $GeneratedDatRoot) {
    $GeneratedDatFiles = @(Get-ChildItem -Path $GeneratedDatRoot -Filter "*.dat" -File)
}

if ($GeneratedDatFiles.Count -eq 0) {
    Write-Warning "No generated DAT files found at $GeneratedDatRoot. Run .\build-dat.ps1 or pass -BuildDat/-RebuildDat when invoking .\build.ps1 before packaging drop data assets."
} else {
    foreach ($DatFile in $GeneratedDatFiles) {
        Copy-Item -Force $DatFile.FullName (Join-Path $DropData $DatFile.Name)
    }
}

$StagedSdlDlls = @(Get-ChildItem -Path $DropRoot -Filter "SDL3.dll" -File -Recurse -ErrorAction SilentlyContinue)
if ($StagedSdlDlls.Count -gt 0) {
    $StagedSdlDllList = ($StagedSdlDlls | ForEach-Object { $_.FullName }) -join ", "
    throw "SDL3 static-link packaging check failed; remove staged SDL3.dll file(s): $StagedSdlDllList"
}

Write-Host "Drop prepared at $DropRoot"