[CmdletBinding()]
param(
    [string]$OS = 'Windows',

    [Alias('Teardown')]
    [switch]$Clean,

    [switch]$DestroyVm,

    [switch]$Force,

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$RemainingArguments
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$LinuxBuildRoot = Join-Path $RepoRoot 'build\linux'
$LinuxConfigPath = Join-Path $LinuxBuildRoot 'config.json'
$LinuxContractPath = Join-Path $LinuxBuildRoot 'LinuxBuildContract.ps1'
$HostPrerequisiteConfigPath = Join-Path $LinuxBuildRoot 'host-prerequisites.json'
$HostPrerequisiteScriptPath = Join-Path $LinuxBuildRoot 'WindowsHostPrerequisites.ps1'
$UbuntuBaseBoxScriptPath = Join-Path $LinuxBuildRoot 'UbuntuBaseBox.ps1'
$LinuxDevelopmentVmScriptPath = Join-Path $LinuxBuildRoot 'LinuxDevelopmentVm.ps1'
$LinuxSourceSyncScriptPath = Join-Path $LinuxBuildRoot 'LinuxSourceSync.ps1'
$Remaining = @($RemainingArguments | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })

if ($OS -in @('--OS', '-OS')) {
    if ($Remaining.Count -eq 0) {
        throw 'The --OS spelling requires exactly one value: --OS Windows or --OS Linux'
    }
    $OS = $Remaining[0]
    $Remaining = @($Remaining | Select-Object -Skip 1)
} elseif ($OS.StartsWith('--OS=')) {
    $OS = $OS.Substring(5)
}

$normalizedRemaining = [System.Collections.Generic.List[string]]::new()
for ($index = 0; $index -lt $Remaining.Count; $index++) {
    $argument = $Remaining[$index]
    if ($argument -in @('--OS', '-OS')) {
        $index++
        if ($index -ge $Remaining.Count) {
            throw 'The --OS spelling requires exactly one value: --OS Windows or --OS Linux'
        }
        $OS = $Remaining[$index]
    } elseif ($argument.StartsWith('--OS=')) {
        $OS = $argument.Substring(5)
    } else {
        $normalizedRemaining.Add($argument)
    }
}
$Remaining = @($normalizedRemaining)

if ($OS -notin @('Windows', 'Linux')) {
    throw "Unsupported OS '$OS'. Expected Windows or Linux."
}

foreach ($argument in $Remaining) {
    if ([string]::IsNullOrWhiteSpace($argument)) {
        continue
    }
    switch ($argument) {
        '-Clean' { $Clean = $true }
        '--Clean' { $Clean = $true }
        '-DestroyVm' { $DestroyVm = $true }
        '--DestroyVm' { $DestroyVm = $true }
        '-Force' { $Force = $true }
        '--Force' { $Force = $true }
        default { throw "Unexpected argument '$argument'." }
    }
}

if ($OS -eq 'Windows' -and ($DestroyVm -or $Force)) {
    throw '-DestroyVm and -Force apply only when -OS Linux is selected.'
}
if ($Force -and -not $DestroyVm) {
    throw '-Force is valid only with -DestroyVm.'
}
if ($DestroyVm -and -not $Force) {
    throw 'Destroying the Linux VM requires explicit confirmation with -Force (or --Force in the compatibility spelling).'
}
if ($DestroyVm -and $Clean) {
    throw '-Clean and -DestroyVm are separate operations and cannot be combined.'
}

if ($OS -eq 'Linux') {
    if (-not (Test-Path -LiteralPath $LinuxContractPath -PathType Leaf)) {
        throw "Linux build contract not found: $LinuxContractPath"
    }
    . $LinuxContractPath
    $vmWorkRoot = Join-Path $RepoRoot '_local\linux\development-vm'
    $vmSettingsPath = Join-Path $vmWorkRoot 'settings.json'
    $localOverrides = @{}
    if (Test-Path -LiteralPath $LinuxDevelopmentVmScriptPath -PathType Leaf) {
        . $LinuxDevelopmentVmScriptPath
        $localOverrides = Get-LocalDevelopmentVmOverrides -Path $vmSettingsPath
    }
    $linuxConfig = Get-LinuxBuildConfiguration -ConfigPath $LinuxConfigPath -Overrides $localOverrides
    $exitStates = Get-LinuxBuildExitStates -ConfigPath $LinuxConfigPath
    if (-not (Test-Path -LiteralPath $HostPrerequisiteScriptPath -PathType Leaf)) {
        throw "Windows host prerequisite implementation not found: $HostPrerequisiteScriptPath"
    }
    . $HostPrerequisiteScriptPath
    $hostWorkRoot = Join-Path $RepoRoot '_local\linux\host-prerequisites'
    New-Item -ItemType Directory -Force -Path $hostWorkRoot | Out-Null
    $result = Initialize-WindowsHostPrerequisites `
        -ConfigPath $HostPrerequisiteConfigPath `
        -RepoRoot $RepoRoot `
        -WorkRoot $hostWorkRoot
    Write-Host $result.Message
    if ($result.State -eq 'restartRequired') {
        exit $exitStates.RestartRequired
    }
    if ($result.State -notin @('ready', 'repaired')) {
        throw "Unknown host prerequisite state '$($result.State)'."
    }
    if (-not (Test-Path -LiteralPath $UbuntuBaseBoxScriptPath -PathType Leaf)) {
        throw "Ubuntu base-box implementation not found: $UbuntuBaseBoxScriptPath"
    }

    $hostConfig = Read-HostPrerequisiteConfig -Path $HostPrerequisiteConfigPath
    $vagrantPackage = $hostConfig.wingetPackages | Where-Object name -eq 'Vagrant'
    $packerPackage = $hostConfig.wingetPackages | Where-Object name -eq 'Packer'
    $virtualBoxPackage = $hostConfig.wingetPackages | Where-Object name -eq 'VirtualBox'
    $vagrant = Resolve-HostExecutable -Command $vagrantPackage.command -CandidatePaths $vagrantPackage.candidatePaths
    $vboxManage = Resolve-HostExecutable -Command $virtualBoxPackage.command -CandidatePaths $virtualBoxPackage.candidatePaths
    $networkDefaults = Resolve-DevelopmentVmNetworkDefaults -Configuration $linuxConfig -VBoxManage $vboxManage
    if ($networkDefaults.Count -gt 0) {
        foreach ($entry in $networkDefaults.GetEnumerator()) {
            $localOverrides[$entry.Key] = $entry.Value
        }
        $linuxConfig = Get-LinuxBuildConfiguration -ConfigPath $LinuxConfigPath -Overrides $localOverrides
    }
    if ($DestroyVm) {
        $destroyResult = Remove-LinuxDevelopmentVm `
            -Vagrant $vagrant `
            -VagrantRoot $LinuxBuildRoot `
            -WorkRoot $vmWorkRoot `
            -Configuration $linuxConfig
        Write-Host $destroyResult.Message
        if ($destroyResult.State -eq 'repaired') {
            exit $exitStates.Repaired
        }
        exit $exitStates.Ready
    }
    if ($Clean) {
        $cleanResult = Clear-LinuxDevelopmentBuildState `
            -Vagrant $vagrant `
            -VagrantRoot $LinuxBuildRoot `
            -WorkRoot $vmWorkRoot `
            -RepoRoot $RepoRoot `
            -Configuration $linuxConfig
        Write-Host $cleanResult.Message
        exit $exitStates.Repaired
    }

    $packer = Resolve-HostExecutable -Command $packerPackage.command -CandidatePaths $packerPackage.candidatePaths
    $curl = Resolve-HostExecutable -Command 'curl.exe' -CandidatePaths @(
        (Join-Path $env:SystemRoot 'System32\curl.exe')
    )
    if (-not $curl) {
        throw 'Windows curl.exe is required to download the checksum-pinned Ubuntu ISO.'
    }

    . $UbuntuBaseBoxScriptPath
    $boxWorkRoot = Join-Path $RepoRoot '_local\linux\ubuntu-base-box'
    $boxResult = Initialize-UbuntuBaseBox `
        -RepoRoot $RepoRoot `
        -WorkRoot $boxWorkRoot `
        -Packer $packer `
        -Vagrant $vagrant `
        -Curl $curl
    Write-Host $boxResult.Message
    if (-not (Test-Path -LiteralPath $LinuxDevelopmentVmScriptPath -PathType Leaf)) {
        throw "Linux development VM implementation not found: $LinuxDevelopmentVmScriptPath"
    }
    if (-not (Test-Path -LiteralPath $LinuxSourceSyncScriptPath -PathType Leaf)) {
        throw "Linux SSH connectivity implementation not found: $LinuxSourceSyncScriptPath"
    }
    . $LinuxSourceSyncScriptPath
    $msysPaths = Get-MsysPaths -Config $hostConfig
    $vmResult = Initialize-LinuxDevelopmentVm `
        -Vagrant $vagrant `
        -VBoxManage $vboxManage `
        -VagrantRoot $LinuxBuildRoot `
        -WorkRoot $vmWorkRoot `
        -Configuration $linuxConfig `
        -MsysPaths $msysPaths
    Write-Host $vmResult.Message
    if ($result.State -eq 'repaired' -or $boxResult.State -eq 'repaired' -or $vmResult.State -eq 'repaired') {
        exit $exitStates.Repaired
    }
    exit $exitStates.Ready
}

$VendorRoot = Join-Path $RepoRoot 'vendor'
$Sdl3Root = Join-Path $VendorRoot 'SDL3'
$Sdl3SysRoot = Join-Path $Sdl3Root 'sdl3-sys-0.6.2+SDL-3.4.4-spacefix'
$JemallocRoot = Join-Path $VendorRoot 'tikv-jemalloc-sys'
$XkbcommonDlRoot = Join-Path $VendorRoot 'xkbcommon-dl'
$VendorPatchRoot = Join-Path $RepoRoot 'build\vendor-patches'
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
xkbcommon-dl = "=0.4.2"
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

function Read-VendorPins {
    param([Parameter(Mandatory = $true)][string]$Path)

    $pins = @{}
    foreach ($line in Get-Content -LiteralPath $Path) {
        if ($line -match '^\s*([A-Z0-9_]+)=(.*)$') {
            $pins[$Matches[1]] = $Matches[2].Trim()
        }
    }

    return $pins
}

# Mirrors hydrate_xkbcommon_dl in build/linux/sync/hydrate-linux-source.sh.
function Initialize-XkbcommonDlSource {
    param([Parameter(Mandatory = $true)][string]$DestinationPath)

    $pins = Read-VendorPins -Path (Join-Path $VendorPatchRoot 'xkbcommon-dl.env')
    $patchFile = Join-Path $VendorPatchRoot $pins.XKBCOMMON_DL_PATCH
    $stampFile = Join-Path $DestinationPath '.flutz-hydrated'
    $patchHash = (Get-FileHash -LiteralPath $patchFile -Algorithm SHA256).Hash.ToLowerInvariant()
    $stamp = "xkbcommon-dl=$($pins.XKBCOMMON_DL_VERSION) patch=$patchHash libxkbcommon=$($pins.LIBXKBCOMMON_SHA256)"
    if ((Test-Path -LiteralPath $stampFile) -and ((Get-Content -LiteralPath $stampFile -Raw).Trim() -eq $stamp)) {
        return
    }

    Remove-PathIfPresent -Path $DestinationPath
    Copy-CrateSource -CrateName 'xkbcommon-dl' -Version $pins.XKBCOMMON_DL_VERSION -DestinationPath $DestinationPath

    # --directory keeps patch paths relative to the crate even though vendor/ sits inside this repository.
    & git -C $RepoRoot apply --directory=vendor/xkbcommon-dl $patchFile
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to apply $patchFile to $DestinationPath"
    }

    $tarball = Join-Path ([System.IO.Path]::GetTempPath()) "libxkbcommon-$($pins.LIBXKBCOMMON_VERSION).tar.gz"
    try {
        Invoke-WebRequest -Uri $pins.LIBXKBCOMMON_URL -OutFile $tarball -UseBasicParsing
        $actualHash = (Get-FileHash -LiteralPath $tarball -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $pins.LIBXKBCOMMON_SHA256) {
            throw "libxkbcommon $($pins.LIBXKBCOMMON_VERSION) checksum mismatch: expected $($pins.LIBXKBCOMMON_SHA256), got $actualHash"
        }

        $libRoot = Join-Path $DestinationPath 'libxkbcommon'
        Ensure-Directory -Path $libRoot
        & (Join-Path $env:SystemRoot 'System32\tar.exe') -xzf $tarball --strip-components=1 -C $libRoot
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to extract $tarball"
        }
    } finally {
        Remove-PathIfPresent -Path $tarball
    }

    Set-Content -LiteralPath $stampFile -Value $stamp
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
Initialize-XkbcommonDlSource -DestinationPath $XkbcommonDlRoot

Write-Step 'Verifying expected vendor layout'
foreach ($requiredPath in @(
    (Join-Path $Sdl3SysRoot 'Cargo.toml'),
    (Join-Path $JemallocRoot 'Cargo.toml'),
    (Join-Path $XkbcommonDlRoot 'build.rs'),
    (Join-Path $XkbcommonDlRoot 'libxkbcommon\meson.build')
)) {
    if (-not (Test-Path -LiteralPath $requiredPath)) {
        throw "Expected initialized path missing: $requiredPath"
    }
}

Write-Step "Initialization complete. build.ps1 can now use $Sdl3Root, and build-dat.ps1 can use $DatManifest."
