[CmdletBinding()]
param(
    [Alias('Teardown')]
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$VendorRoot = Join-Path $RepoRoot 'vendor'
$Sdl3Root = Join-Path $VendorRoot 'SDL3'
$Sdl3SysRoot = Join-Path $Sdl3Root 'sdl3-sys-0.6.2+SDL-3.4.4-spacefix'
$JemallocRoot = Join-Path $VendorRoot 'tikv-jemalloc-sys'
$CargoRoot = Join-Path $RepoRoot '.cargo'
$CargoConfig = Join-Path $CargoRoot 'config.toml'
$LocalRoot = Join-Path $RepoRoot '_local'
$ScratchRoot = Join-Path $LocalRoot 'scratch'
$DropsRoot = Join-Path $RepoRoot 'drops'
$DropRoot = Join-Path $DropsRoot 'flutzplayer'
$DropData = Join-Path $DropRoot 'data'
$DropDatPacker = Join-Path $DropRoot 'dat-packer'
$GeneratedDatRoot = Join-Path $LocalRoot 'generated-assets/dat'
$RuntimeTestsRoot = Join-Path $LocalRoot 'runtime-tests'
$RuntimeTestsScripts = Join-Path $RuntimeTestsRoot 'scripts'
$RuntimeTestsMeasurements = Join-Path $RuntimeTestsRoot 'measurements'
$LocalLogsRoot = Join-Path $LocalRoot 'logs'
$LocalScratchRoot = Join-Path $LocalRoot 'scratch'
$AnalyzerTraceRoot = Join-Path $LocalRoot 'analyzer-trace'
$RenderErrorTraceRoot = Join-Path $LocalRoot 'render-error-trace'
$DatManifest = Join-Path $RepoRoot 'assets/dat-manifest.toml'
$CargoLock = Join-Path $RepoRoot 'Cargo.lock'

function Write-Step {
    param([Parameter(Mandatory = $true)][string]$Message)

    Write-Host $Message
}

function Ensure-Directory {
    param([Parameter(Mandatory = $true)][string]$Path)

    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Remove-PathIfPresent {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (Test-Path -LiteralPath $Path) {
        $item = Get-Item -LiteralPath $Path -Force
        if ($item.PSIsContainer) {
            for ($attempt = 0; $attempt -lt 3 -and (Test-Path -LiteralPath $Path); $attempt++) {
                $entries = @(Get-ChildItem -LiteralPath $Path -Force -Recurse -ErrorAction SilentlyContinue |
                    Sort-Object { $_.FullName.Length } -Descending)

                foreach ($entry in $entries) {
                    try {
                        if ($entry.PSIsContainer) {
                            [System.IO.Directory]::SetAttributes($entry.FullName, [System.IO.FileAttributes]::Normal)
                        } else {
                            [System.IO.File]::SetAttributes($entry.FullName, [System.IO.FileAttributes]::Normal)
                        }
                    } catch {
                    }

                    try {
                        Remove-Item -Force -LiteralPath $entry.FullName -Recurse -ErrorAction Stop
                    } catch {
                    }
                }

                try {
                    [System.IO.Directory]::SetAttributes($Path, [System.IO.FileAttributes]::Normal)
                } catch {
                }

                try {
                    Remove-Item -Force -LiteralPath $Path -Recurse -ErrorAction Stop
                } catch {
                }
            }

            if (Test-Path -LiteralPath $Path) {
                Write-Warning "Could not remove ${Path}: path is still in use"
            }
        } else {
            try {
                Remove-Item -Force -LiteralPath $Path -ErrorAction Stop
            } catch {
                Write-Warning "Could not remove ${Path}: $($_.Exception.Message)"
            }
        }
    }
}

function Find-ShellExecutable {
    $candidatePaths = @()

    if ($env:CONFIG_SHELL) {
        $candidatePaths += $env:CONFIG_SHELL
    }

    if ($env:MSYS2_ROOT) {
        $candidatePaths += (Join-Path $env:MSYS2_ROOT 'usr\bin\sh.exe')
        $candidatePaths += (Join-Path $env:MSYS2_ROOT 'usr\bin\bash.exe')
    }

    $candidatePaths += @(
        'C:\msys64\usr\bin\sh.exe',
        'C:\msys64\usr\bin\bash.exe',
        'C:\Program Files\Git\bin\sh.exe',
        'C:\Program Files\Git\bin\bash.exe',
        'C:\Program Files\Git\usr\bin\sh.exe',
        'C:\Program Files\Git\usr\bin\bash.exe',
        'C:\Program Files (x86)\Git\bin\sh.exe',
        'C:\Program Files (x86)\Git\bin\bash.exe',
        'C:\Program Files (x86)\Git\usr\bin\sh.exe',
        'C:\Program Files (x86)\Git\usr\bin\bash.exe'
    )

    foreach ($candidatePath in $candidatePaths) {
        if ($candidatePath -and (Test-Path -LiteralPath $candidatePath)) {
            return $candidatePath
        }
    }

    throw 'Could not find a usable sh.exe or bash.exe for jemalloc configure.'
}

function Get-RegistrySourceRoot {
    $registrySrcRoot = Join-Path $env:USERPROFILE '.cargo\registry\src'
    if (-not (Test-Path -LiteralPath $registrySrcRoot)) {
        throw "Cargo registry source cache not found: $registrySrcRoot"
    }

    return $registrySrcRoot
}

function Ensure-CrateSourcesFetched {
    $fetchRoot = Join-Path ([System.IO.Path]::GetTempPath()) 'flutz-build-init-fetch'
    Remove-PathIfPresent -Path $fetchRoot
    Ensure-Directory -Path $fetchRoot
    Ensure-Directory -Path (Join-Path $fetchRoot 'src')

    $manifestPath = Join-Path $fetchRoot 'Cargo.toml'
    $mainPath = Join-Path $fetchRoot 'src/main.rs'
    Set-Content -LiteralPath $manifestPath -Value @'
[package]
name = "flutz_build_init_fetch"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
sdl3-sys = "=0.6.2+SDL-3.4.4"
tikv-jemalloc-sys = "=0.6.1"
'@
    Set-Content -LiteralPath $mainPath -Value 'fn main() {}'

    & cargo fetch --manifest-path $manifestPath
    if ($LASTEXITCODE -ne 0) {
        throw "cargo fetch failed with exit code $LASTEXITCODE"
    }

    Remove-PathIfPresent -Path $fetchRoot
}

function Patch-JemallocBuildScript {
    param([Parameter(Mandatory = $true)][string]$SourceRoot)

    $buildScriptPath = Join-Path $SourceRoot 'build.rs'
    if (-not (Test-Path -LiteralPath $buildScriptPath)) {
        return
    }

    $buildScript = [System.IO.File]::ReadAllText($buildScriptPath)

    $buildScript = [regex]::Replace(
        $buildScript,
        'let mut cmd = Command::new\("sh"\);',
@'
    let shell = if let Some(shell) = read_and_watch_env_os("CONFIG_SHELL") {
        PathBuf::from(shell)
    } else if target.contains("windows") {
        let candidate_paths = [
            r"C:\msys64\usr\bin\sh.exe",
            r"C:\msys64\usr\bin\bash.exe",
            r"C:\Program Files\Git\bin\sh.exe",
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\sh.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\sh.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\usr\bin\sh.exe",
            r"C:\Program Files (x86)\Git\usr\bin\bash.exe",
        ];

        candidate_paths
            .iter()
            .map(PathBuf::from)
            .find(|candidate| candidate.exists())
            .unwrap_or_else(|| PathBuf::from("sh"))
    } else {
        PathBuf::from("sh")
    };
    let shell_dir = shell.parent().map(PathBuf::from);
    if let Some(shell_dir) = shell_dir {
        let mut path = OsString::from(shell_dir);
        if let Some(existing_path) = env::var_os("PATH") {
            path.push(";");
            path.push(existing_path);
        }
        env::set_var("PATH", &path);
        env::set_var("SHELL", &shell);
        env::set_var("MAKESHELL", &shell);
    }
    let mut cmd = Command::new(shell);
'@
    )

    $buildScript = [regex]::Replace(
        $buildScript,
        '(?s)// Make install:\s*run\(make_command\(make, &build_dir, &num_jobs\)\s*\.arg\("install_lib_static"\)\s*\.arg\("install_include"\)\);\s*\s*println!\("cargo:root=\{\}", out_dir\.display\(\)\);',
@'
    // Make install:
    run(make_command(make, &build_dir, &num_jobs)
        .arg("install_lib_static")
        .arg("install_include"));

    if target.contains("windows") {
        let windows_static_lib = build_dir.join("lib").join("jemalloc.lib");
        let expected_static_lib = build_dir.join("lib").join("libjemalloc.a");
        if windows_static_lib.exists() && !expected_static_lib.exists() {
            fs::copy(&windows_static_lib, &expected_static_lib)
                .expect("failed to copy jemalloc static library for GNU linking");
        }
    }

    println!("cargo:root={}", out_dir.display());
'@
    )

    if ($buildScript -notmatch 'libjemalloc\.a') {
        throw 'Failed to patch tikv-jemalloc-sys build.rs for GNU static-library linking.'
    }

    [System.IO.File]::WriteAllText($buildScriptPath, $buildScript)
}

function Copy-CrateSource {
    param(
        [Parameter(Mandatory = $true)][string]$CrateName,
        [Parameter(Mandatory = $true)][string]$Version,
        [Parameter(Mandatory = $true)][string]$DestinationPath
    )

    $destinationManifest = Join-Path $DestinationPath 'Cargo.toml'
    if (Test-Path -LiteralPath $destinationManifest) {
        if ($CrateName -eq 'tikv-jemalloc-sys') {
            Patch-JemallocBuildScript -SourceRoot $DestinationPath
        }

        return
    }

    if (Test-Path -LiteralPath $DestinationPath) {
        Remove-PathIfPresent -Path $DestinationPath
    }

    Ensure-CrateSourcesFetched

    $packageName = "$CrateName-$Version"
    $registryRoot = Get-RegistrySourceRoot
    $packageRoot = Get-ChildItem -LiteralPath $registryRoot -Recurse -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like "$packageName*" } |
        Select-Object -First 1

    if ($null -eq $packageRoot) {
        throw "Could not locate $packageName in the Cargo registry source cache under $registryRoot"
    }

    Ensure-Directory -Path (Split-Path -Parent $DestinationPath)
    Copy-Item -Recurse -Force -LiteralPath $packageRoot.FullName -Destination $DestinationPath

    if ($CrateName -eq 'tikv-jemalloc-sys') {
        Patch-JemallocBuildScript -SourceRoot $DestinationPath
    }
}

function Validate-ManifestSources {
    param([Parameter(Mandatory = $true)][string]$ManifestPath)

    if (-not (Test-Path -LiteralPath $ManifestPath)) {
        throw "Missing DAT manifest: $ManifestPath"
    }

    $missing = @()
    foreach ($line in Get-Content -LiteralPath $ManifestPath) {
        if ($line -match '^\s*source_path\s*=\s*"([^"]+)"\s*$') {
            $candidate = Join-Path $RepoRoot $Matches[1]
            if (-not (Test-Path -LiteralPath $candidate)) {
                $missing += $candidate
            }
        }
    }

    if ($missing.Count -gt 0) {
        $formatted = ($missing | Sort-Object) -join [Environment]::NewLine
        Write-Step "Missing soundfont source file(s) referenced by assets/dat-manifest.toml:`n$formatted"
        Write-Step 'Running soundfonts/get-fonts.ps1 to fetch missing files'
        $getFontsScript = Join-Path $RepoRoot 'soundfonts/get-fonts.ps1'
        try {
            & $getFontsScript
        } catch {
            throw "soundfonts/get-fonts.ps1 failed: $_"
        }

        $stillMissing = $missing | Where-Object { -not (Test-Path -LiteralPath $_) }
        if ($stillMissing.Count -gt 0) {
            $formatted = ($stillMissing | Sort-Object) -join [Environment]::NewLine
            throw "Soundfont source file(s) still missing after running get-fonts.ps1:`n$formatted"
        }
    }
}

function Remove-InitArtifacts {
    Remove-PathIfPresent -Path $VendorRoot
    Remove-PathIfPresent -Path $CargoRoot
    Remove-PathIfPresent -Path $LocalRoot
    Remove-PathIfPresent -Path $DropsRoot
    Remove-PathIfPresent -Path $CargoLock
}

if ($Clean) {
    Write-Step 'Removing build-init customizations'
    Remove-InitArtifacts
    Write-Step 'Workspace reset complete'
    return
}

Write-Step 'Validating checked-in source assets'
Validate-ManifestSources -ManifestPath $DatManifest

Write-Step 'Creating development directories'
foreach ($path in @(
    $CargoRoot,
    $VendorRoot,
    $Sdl3Root,
    $LocalRoot,
    $ScratchRoot,
    $DropsRoot,
    $DropRoot,
    $DropData,
    $DropDatPacker,
    $GeneratedDatRoot,
    $RuntimeTestsRoot,
    $RuntimeTestsScripts,
    $RuntimeTestsMeasurements,
    $LocalLogsRoot,
    $LocalScratchRoot,
    $AnalyzerTraceRoot,
    $RenderErrorTraceRoot
)) {
    Ensure-Directory -Path $path
}

$ShellExecutable = Find-ShellExecutable
Set-Content -LiteralPath $CargoConfig -Value @"
[build]
target-dir = "_local/target"

[env]
CONFIG_SHELL = '$ShellExecutable'
"@

Write-Step 'Hydrating vendor sources'
Copy-CrateSource -CrateName 'sdl3-sys' -Version '0.6.2+SDL-3.4.4' -DestinationPath $Sdl3SysRoot
Copy-CrateSource -CrateName 'tikv-jemalloc-sys' -Version '0.6.1' -DestinationPath $JemallocRoot

Write-Step 'Verifying expected vendor layout'
foreach ($requiredPath in @(
    (Join-Path $Sdl3SysRoot 'Cargo.toml'),
    (Join-Path $JemallocRoot 'Cargo.toml')
)) {
    if (-not (Test-Path -LiteralPath $requiredPath)) {
        throw "Expected initialized path missing: $requiredPath"
    }
}

Write-Step "Initialization complete. build.ps1 can now use $Sdl3Root, and build-dat.ps1 can use $DatManifest."
