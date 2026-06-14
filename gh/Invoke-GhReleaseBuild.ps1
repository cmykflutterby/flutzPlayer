param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Release"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$PrepareVendorScript = Join-Path $PSScriptRoot "Prepare-GhVendor.ps1"
$GhSoundfontScript = Join-Path $PSScriptRoot "Get-GhSoundfonts.ps1"
$DatManifest = Join-Path $RepoRoot "assets/dat-manifest.toml"
$GeneratedDatRoot = Join-Path $RepoRoot "_local/generated-assets/dat"
$GeneratedDatFile = Join-Path $GeneratedDatRoot "assets.dat"
$DropRoot = Join-Path $RepoRoot "drops/flutzplayer"
$DropData = Join-Path $DropRoot "data"
$DropDatPacker = Join-Path $DropRoot "dat-packer"
$TargetRoot = Join-Path $RepoRoot "target"
$DatPackerWrapper = Join-Path $RepoRoot "tools/dat-packer/pack-dat.ps1"

function Initialize-NoSpaceWorkspaceRoot {
    param([Parameter(Mandatory = $true)][string]$ActualRepoRoot)

    if ($ActualRepoRoot -notmatch '\s') {
        return $ActualRepoRoot
    }

    $workspaceAliasRoot = Join-Path ([System.IO.Path]::GetTempPath()) "flutz-gh-workspace"
    if (Test-Path -LiteralPath $workspaceAliasRoot) {
        Remove-Item -Recurse -Force -LiteralPath $workspaceAliasRoot
    }

    New-Item -ItemType Junction -Path $workspaceAliasRoot -Target $ActualRepoRoot | Out-Null
    return $workspaceAliasRoot
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

$RepoRoot = Initialize-NoSpaceWorkspaceRoot -ActualRepoRoot $RepoRoot
    $CargoExe = Resolve-CargoExecutable

Push-Location $RepoRoot
try {
    if (-not (Test-Path -LiteralPath $PrepareVendorScript)) {
        throw "Missing GH vendor bootstrap script: $PrepareVendorScript"
    }
    if (-not (Test-Path -LiteralPath $GhSoundfontScript)) {
        throw "Missing GH soundfont script: $GhSoundfontScript"
    }
    if (-not (Test-Path -LiteralPath $DatManifest)) {
        throw "Missing DAT manifest: $DatManifest"
    }

    Write-Host "Hydrating GH vendored crate sources"
    & $PrepareVendorScript
    if ($LASTEXITCODE -ne 0) {
        throw "Prepare-GhVendor.ps1 failed with exit code $LASTEXITCODE"
    }

    $hasSoundfonts = -not [string]::IsNullOrWhiteSpace($env:FLUTZ_SOUNDFONT_URL)
    $soundfontDir = Join-Path $RepoRoot "soundfonts"

    if ($hasSoundfonts) {
        Write-Host "Acquiring soundfont assets"
        & $GhSoundfontScript -DestinationDirectory $soundfontDir
        if ($LASTEXITCODE -ne 0) {
            throw "Get-GhSoundfonts.ps1 failed with exit code $LASTEXITCODE"
        }
    } else {
        Write-Host "FLUTZ_SOUNDFONT_URL not set; skipping soundfont download and DAT packing (binary-only build)"
    }

    if (Test-Path -LiteralPath $DropRoot) {
        Remove-Item -Recurse -Force -LiteralPath $DropRoot
    }
    New-Item -ItemType Directory -Force -Path $DropRoot | Out-Null
    New-Item -ItemType Directory -Force -Path $DropData | Out-Null
    New-Item -ItemType Directory -Force -Path $DropDatPacker | Out-Null
    New-Item -ItemType Directory -Force -Path $GeneratedDatRoot | Out-Null

    $profileFolder = if ($Configuration -eq "Release") { "release" } else { "debug" }
    $cargoModeArgs = if ($Configuration -eq "Release") { @("--release") } else { @() }

    if ($hasSoundfonts) {
        Write-Host "Packing DAT assets"
        $datPackArgs = @("run", "-p", "flutz_soundfont_tools") + $cargoModeArgs + @(
            "--",
            "--pack",
            "--input", $DatManifest,
            "--output", $GeneratedDatFile,
            "--base-dir", $RepoRoot
        )
        Invoke-Cargo -CargoExe $CargoExe -Args $datPackArgs
    }

    Write-Host "Building flutzplayer binary"
    $appBuildArgs = @("build", "-p", "flutz_app", "--features", "jemalloc-memory") + $cargoModeArgs
    Invoke-Cargo -CargoExe $CargoExe -Args $appBuildArgs

    Write-Host "Building DAT packer binary"
    $datPackerBuildArgs = @("build", "-p", "flutz_soundfont_tools") + $cargoModeArgs
    Invoke-Cargo -CargoExe $CargoExe -Args $datPackerBuildArgs

    $builtExe = Join-Path $TargetRoot "$profileFolder/flutzplayer.exe"
    $builtDatPacker = Join-Path $TargetRoot "$profileFolder/flutz_soundfont_tools.exe"
    if (-not (Test-Path -LiteralPath $builtExe)) {
        throw "Built app binary not found: $builtExe"
    }
    if (-not (Test-Path -LiteralPath $builtDatPacker)) {
        throw "Built DAT packer binary not found: $builtDatPacker"
    }

    Copy-Item -Force -LiteralPath $builtExe -Destination (Join-Path $DropRoot "flutzplayer.exe")
    Copy-Item -Force -LiteralPath $builtDatPacker -Destination (Join-Path $DropDatPacker "dat-packer.exe")

    if (Test-Path -LiteralPath $DatPackerWrapper) {
        Copy-Item -Force -LiteralPath $DatPackerWrapper -Destination (Join-Path $DropDatPacker "pack-dat.ps1")
    }
    Copy-Item -Force -LiteralPath $DatManifest -Destination (Join-Path $DropDatPacker "dat-manifest.toml")

    $generatedDatFiles = @(Get-ChildItem -LiteralPath $GeneratedDatRoot -Filter "*.dat" -File -ErrorAction SilentlyContinue)
    if ($generatedDatFiles.Count -eq 0 -and $hasSoundfonts) {
        throw "No DAT files generated in $GeneratedDatRoot"
    }
    foreach ($datFile in $generatedDatFiles) {
        Copy-Item -Force -LiteralPath $datFile.FullName -Destination (Join-Path $DropData $datFile.Name)
    }

    $stagedSdlDlls = @(Get-ChildItem -Path $DropRoot -Filter "SDL3.dll" -File -Recurse -ErrorAction SilentlyContinue)
    if ($stagedSdlDlls.Count -gt 0) {
        $stagedSdlDllList = ($stagedSdlDlls | ForEach-Object { $_.FullName }) -join ", "
        throw "SDL3 static-link packaging check failed; remove staged SDL3.dll file(s): $stagedSdlDllList"
    }

    Write-Host "Release drop prepared at $DropRoot"
}
finally {
    Pop-Location
}
