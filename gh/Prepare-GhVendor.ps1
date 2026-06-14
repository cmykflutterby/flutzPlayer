Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$VendorRoot = Join-Path $RepoRoot "vendor"
$Sdl3Root = Join-Path $VendorRoot "SDL3"
$JemallocRoot = Join-Path $VendorRoot "tikv-jemalloc-sys"
$Sdl3SysVersion = "0.6.2+SDL-3.4.4"
$Sdl3SrcVersion = "3.4.4"
$JemallocVersion = "0.6.1"
$Sdl3SysDestination = Join-Path $Sdl3Root "sdl3-sys-0.6.2+SDL-3.4.4-spacefix"
$Sdl3SrcDestination = Join-Path $Sdl3Root "sdl3-src-3.4.4"

function Ensure-Directory {
    param([Parameter(Mandatory = $true)][string]$Path)
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Resolve-CargoExecutable {
    $cargoCommand = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $cargoCommand) {
        throw 'Could not locate cargo in the active environment.'
    }

    return $cargoCommand.Source
}

function Resolve-ToolExecutable {
    param(
        [Parameter(Mandatory = $true)][string[]]$Names,
        [Parameter(Mandatory = $true)][string]$ToolDescription,
        [string]$PreferredRoot
    )

    if (-not [string]::IsNullOrWhiteSpace($PreferredRoot) -and (Test-Path -LiteralPath $PreferredRoot)) {
        foreach ($name in $Names) {
            $preferredCandidate = Join-Path $PreferredRoot $name
            if (Test-Path -LiteralPath $preferredCandidate) {
                return $preferredCandidate
            }
        }
    }

    foreach ($name in $Names) {
        $command = @(Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1)
        if ($command.Count -gt 0) {
            return $command[0].Source
        }
    }

    throw "Could not locate $ToolDescription in the active environment. Tried: $($Names -join ', ')"
}

function Remove-PathIfPresent {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (Test-Path -LiteralPath $Path) {
        Remove-Item -Recurse -Force -LiteralPath $Path
    }
}

function Get-RegistrySourceRoot {
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
    $registrySrcRoot = Join-Path $cargoHome "registry/src"
    if (-not (Test-Path -LiteralPath $registrySrcRoot)) {
        throw "Cargo registry source cache not found: $registrySrcRoot"
    }

    return $registrySrcRoot
}

function Ensure-CrateSourcesFetched {
    $fetchRoot = Join-Path ([System.IO.Path]::GetTempPath()) "flutz-gh-vendor-fetch"
    Remove-PathIfPresent -Path $fetchRoot
    Ensure-Directory -Path $fetchRoot
    Ensure-Directory -Path (Join-Path $fetchRoot "src")

    $manifestPath = Join-Path $fetchRoot "Cargo.toml"
    $mainPath = Join-Path $fetchRoot "src/main.rs"
    Set-Content -LiteralPath $manifestPath -Value @"
[package]
name = "flutz_gh_vendor_fetch"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
sdl3-sys = "=$Sdl3SysVersion"
sdl3-src = "=$Sdl3SrcVersion"
tikv-jemalloc-sys = "=$JemallocVersion"
"@
    Set-Content -LiteralPath $mainPath -Value "fn main() {}"

    $cargoExe = Resolve-CargoExecutable
    & $cargoExe fetch --manifest-path $manifestPath
    if ($LASTEXITCODE -ne 0) {
        throw "$cargoExe fetch failed with exit code $LASTEXITCODE"
    }

    Remove-PathIfPresent -Path $fetchRoot
}

function Patch-JemallocBuildScript {
    param([Parameter(Mandatory = $true)][string]$SourceRoot)

    $buildScriptPath = Join-Path $SourceRoot "build.rs"
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

function Patch-Sdl3SysManifest {
    param(
        [Parameter(Mandatory = $true)][string]$Sdl3SysRoot,
        [Parameter(Mandatory = $true)][string]$Sdl3SrcRoot
    )

    $manifestPath = Join-Path $Sdl3SysRoot "Cargo.toml"
    if (-not (Test-Path -LiteralPath $manifestPath)) {
        throw "Expected sdl3-sys Cargo.toml not found: $manifestPath"
    }

    $manifest = [System.IO.File]::ReadAllText($manifestPath)
    $sdl3SrcPathToml = ($Sdl3SrcRoot -replace '\\', '/')

    $replacement = @"
[build-dependencies.sdl3-src]
path = "$sdl3SrcPathToml"
optional = true
"@

    if ($manifest -match '(?ms)^\[build-dependencies\.sdl3-src\]') {
        $updated = [regex]::Replace(
            $manifest,
            '(?ms)^\[build-dependencies\.sdl3-src\]\r?\n.*?(?=^\[|\z)',
            "$replacement`r`n"
        )
    } elseif ($manifest -match '(?ms)^\[build-dependencies\]') {
        $updated = [regex]::Replace(
            $manifest,
            '(?ms)^\[build-dependencies\]\r?\n',
            "[build-dependencies]`r`n`r`n$replacement`r`n",
            1
        )
    } else {
        $updated = "$manifest`r`n$replacement`r`n"
    }

    if ($updated -notmatch '(?ms)^\[build-dependencies\.sdl3-src\]\r?\n\s*path\s*=\s*"') {
        throw 'Failed to patch sdl3-sys Cargo.toml with a vendored sdl3-src path.'
    }

    [System.IO.File]::WriteAllText($manifestPath, $updated)
}

function Copy-CrateSource {
    param(
        [Parameter(Mandatory = $true)][string]$CrateName,
        [Parameter(Mandatory = $true)][string]$Version,
        [Parameter(Mandatory = $true)][string]$DestinationPath
    )

    $destinationManifest = Join-Path $DestinationPath "Cargo.toml"
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

    $candidates = @(Get-ChildItem -LiteralPath $registryRoot -Recurse -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $packageName -or $_.Name -like "$packageName+*" })

    if ($candidates.Count -eq 0) {
        throw "Could not locate $packageName in the Cargo registry source cache under $registryRoot"
    }

    $preferredCandidates = @($candidates | Where-Object { $_.FullName -match 'index\.crates\.io-' })
    $selectionPool = if ($preferredCandidates.Count -gt 0) { $preferredCandidates } else { $candidates }

    $packageRoot = $selectionPool |
        Sort-Object @{ Expression = { if ($_.Name -eq $packageName) { 0 } else { 1 } } }, @{ Expression = { $_.Name } } |
        Select-Object -First 1

    if ($null -eq $packageRoot) {
        throw "Could not locate $packageName in the Cargo registry source cache under $registryRoot"
    }

    if ($candidates.Count -gt 1) {
        $candidateList = ($candidates | ForEach-Object { $_.FullName }) -join '; '
        Write-Host "Multiple registry candidates found for $packageName; selected $($packageRoot.FullName). Candidates: $candidateList"
    }

    Ensure-Directory -Path (Split-Path -Parent $DestinationPath)
    Copy-Item -Recurse -Force -LiteralPath $packageRoot.FullName -Destination $DestinationPath

    if ($CrateName -eq 'tikv-jemalloc-sys') {
        Patch-JemallocBuildScript -SourceRoot $DestinationPath
    }
}

if ((Test-Path -LiteralPath (Join-Path $Sdl3SysDestination "Cargo.toml")) -and (Test-Path -LiteralPath (Join-Path $JemallocRoot "Cargo.toml"))) {
    Write-Host "Using existing vendored SDL3 source: $Sdl3SysDestination"
    if (-not (Test-Path -LiteralPath (Join-Path $Sdl3SrcDestination "Cargo.toml"))) {
        Copy-CrateSource -CrateName 'sdl3-src' -Version $Sdl3SrcVersion -DestinationPath $Sdl3SrcDestination
    }
    Patch-Sdl3SysManifest -Sdl3SysRoot $Sdl3SysDestination -Sdl3SrcRoot $Sdl3SrcDestination
    exit 0
}

Ensure-Directory -Path $VendorRoot
Ensure-Directory -Path $Sdl3Root
Ensure-Directory -Path $JemallocRoot

Copy-CrateSource -CrateName 'sdl3-sys' -Version $Sdl3SysVersion -DestinationPath $Sdl3SysDestination
Copy-CrateSource -CrateName 'sdl3-src' -Version $Sdl3SrcVersion -DestinationPath $Sdl3SrcDestination
Copy-CrateSource -CrateName 'tikv-jemalloc-sys' -Version $JemallocVersion -DestinationPath $JemallocRoot
Patch-Sdl3SysManifest -Sdl3SysRoot $Sdl3SysDestination -Sdl3SrcRoot $Sdl3SrcDestination

foreach ($requiredPath in @(
    (Join-Path $Sdl3SysDestination 'Cargo.toml'),
    (Join-Path $Sdl3SrcDestination 'Cargo.toml'),
    (Join-Path $JemallocRoot 'Cargo.toml'),
    (Join-Path $JemallocRoot 'build.rs')
)) {
    if (-not (Test-Path -LiteralPath $requiredPath)) {
        throw "Vendoring step did not produce expected path: $requiredPath"
    }
}

Write-Host "Vendored SDL3 source ready at $Sdl3SysDestination"
Write-Host "Vendored jemalloc source ready at $JemallocRoot"

# Write .cargo/config.toml so cargo uses the GNU target and the discovered shell.
# The shell path is required for jemalloc's autoconf configure step.

$cargoConfigDir = Join-Path $RepoRoot '.cargo'
$cargoConfigPath = Join-Path $cargoConfigDir 'config.toml'
Ensure-Directory -Path $cargoConfigDir

$shellRoot = $env:FLUTZ_GNU_SHELL_ROOT
$toolRoot = $env:FLUTZ_GNU_TOOL_ROOT
$shellPath = Resolve-ToolExecutable -Names @('sh.exe', 'bash.exe', 'sh', 'bash') -ToolDescription 'a POSIX shell (sh.exe or bash.exe)' -PreferredRoot $shellRoot
$linkerPath = Resolve-ToolExecutable -Names @('gcc.exe', 'x86_64-w64-mingw32-gcc.exe', 'gcc') -ToolDescription 'a GNU C compiler (gcc.exe)' -PreferredRoot $toolRoot
$arPath = Resolve-ToolExecutable -Names @('ar.exe', 'gcc-ar.exe', 'x86_64-w64-mingw32-ar.exe') -ToolDescription 'a GNU archiver (ar.exe)' -PreferredRoot $toolRoot

# Forward slashes are required in TOML strings passed to cargo
$linkerPathToml = $linkerPath -replace '\\', '/'
$shellPathToml = $shellPath -replace '\\', '/'
$arPathToml = $arPath -replace '\\', '/'

Set-Content -LiteralPath $cargoConfigPath -Value @"
[build]
target-dir = "target"

[target.x86_64-pc-windows-gnu]
linker = "$linkerPathToml"

[env]
CC = "$linkerPathToml"
AR = "$arPathToml"
CONFIG_SHELL = "$shellPathToml"
"@

Write-Host "Wrote .cargo/config.toml (GNU target, MSYS2 shell)"
