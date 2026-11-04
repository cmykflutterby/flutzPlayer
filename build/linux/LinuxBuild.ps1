function Clear-LinuxPlainDropContents {
    param([Parameter(Mandatory = $true)][string]$DropRoot)

    if (-not (Test-Path -LiteralPath $DropRoot -PathType Container)) {
        return
    }

    $dataRoot = Join-Path $DropRoot 'data'
    Get-ChildItem -LiteralPath $DropRoot -Force |
        Where-Object Name -cne 'data' |
        Remove-Item -Recurse -Force
    if (Test-Path -LiteralPath $dataRoot -PathType Container) {
        Get-ChildItem -LiteralPath $dataRoot -Force | Remove-Item -Recurse -Force
    }
}

function Invoke-LinuxBuild {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][ValidateSet('Linux', 'Linux-AppImage')][string]$OS,
        [Parameter(Mandatory = $true)][ValidateSet('Debug', 'Release')][string]$Configuration,
        [switch]$SkipBuild,
        [switch]$RebuildDat,
        [switch]$BuildDat,
        [switch]$CleanDrop,
        [switch]$NoJemallocMemory,
        [ValidateRange(60, 86400)][int]$CommandTimeoutSeconds = 14400,
        [ValidateRange(60, 86400)][int]$TransferTimeoutSeconds = 1800,
        [ValidateRange(5, 300)][int]$SshTimeoutSeconds = 30
    )

    $LinuxBuildRoot = Join-Path $RepoRoot 'build/linux'
    $LinuxConfigPath = Join-Path $LinuxBuildRoot 'config.json'
    $LinuxContractPath = Join-Path $LinuxBuildRoot 'LinuxBuildContract.ps1'
    $buildLogRoot = Join-Path $RepoRoot '_local/linux/build-output'

    if ($OS -in @("Linux", "Linux-AppImage")) {
        if (-not (Test-Path -LiteralPath $LinuxContractPath -PathType Leaf)) {
            throw "Linux build contract not found: $LinuxContractPath"
        }
        . $LinuxContractPath
        . (Join-Path $LinuxBuildRoot "LinuxDevelopmentVm.ps1")
        . (Join-Path $LinuxBuildRoot "LinuxSourceSync.ps1")
        . (Join-Path $LinuxBuildRoot "WindowsHostPrerequisites.ps1")
        . (Join-Path $LinuxBuildRoot "LinuxDat.ps1")
        if ($OS -eq "Linux-AppImage") {
            . (Join-Path $LinuxBuildRoot "LinuxAppImage.ps1")
        }

        $vmWorkRoot = Join-Path $RepoRoot "_local\linux\development-vm"
        $syncWorkRoot = Join-Path $RepoRoot "_local\linux\source-sync"
        $localOverrides = Get-LocalDevelopmentVmOverrides -Path (Join-Path $vmWorkRoot "settings.json")
        $linuxConfig = Get-LinuxBuildConfiguration -ConfigPath $LinuxConfigPath -Overrides $localOverrides
        $hostConfig = Read-HostPrerequisiteConfig -Path (Join-Path $LinuxBuildRoot "host-prerequisites.json")
        $vagrantPackage = $hostConfig.wingetPackages | Where-Object name -eq 'Vagrant'
        $virtualBoxPackage = $hostConfig.wingetPackages | Where-Object name -eq 'VirtualBox'
        $vagrant = Resolve-HostExecutable -Command $vagrantPackage.command -CandidatePaths $vagrantPackage.candidatePaths
        $vboxManage = Resolve-HostExecutable -Command $virtualBoxPackage.command -CandidatePaths $virtualBoxPackage.candidatePaths
        if (-not $vagrant -or -not $vboxManage) {
            throw "Vagrant and VirtualBox are required. Run build-init.ps1 --OS Linux first."
        }

        $networkDefaults = Resolve-DevelopmentVmNetworkDefaults -Configuration $linuxConfig -VBoxManage $vboxManage
        if ($networkDefaults.Count -gt 0) {
            foreach ($entry in $networkDefaults.GetEnumerator()) {
                $localOverrides[$entry.Key] = $entry.Value
            }
            $linuxConfig = Get-LinuxBuildConfiguration -ConfigPath $LinuxConfigPath -Overrides $localOverrides
        }

        $credentialPath = Join-Path $vmWorkRoot "credentials\rdp-test-account.json"
        if (-not (Test-Path -LiteralPath $credentialPath -PathType Leaf)) {
            throw "Linux VM credentials are missing. Run build-init.ps1 --OS Linux first: $credentialPath"
        }

        $environment = Get-DevelopmentVmEnvironment `
            -Configuration $linuxConfig `
            -CredentialPath $credentialPath `
            -Vagrant $vagrant
        $machine = Get-DevelopmentVmMachineState `
            -Vagrant $vagrant `
            -VagrantRoot $LinuxBuildRoot `
            -Environment $environment
        if ($machine.State -ne 'running') {
            throw "The Linux development VM is '$($machine.State)', not running. Run build-init.ps1 --OS Linux first."
        }

        $msysPaths = Get-MsysPaths -Config $hostConfig
        $sshConfiguration = Resolve-LinuxSshConfiguration `
            -Vagrant $vagrant `
            -VagrantRoot $LinuxBuildRoot `
            -Environment $environment `
            -OutputPath (Join-Path $syncWorkRoot "ssh-config") `
            -MsysPaths $msysPaths `
            -VBoxManage $vboxManage `
            -MachineId $machine.MachineId `
            -TrustedCidr $linuxConfig.RdpSourceCidr `
            -ConnectTimeoutSeconds $SshTimeoutSeconds
        $sshConfiguration | Add-Member -NotePropertyName CommandTimeoutSeconds -NotePropertyValue $CommandTimeoutSeconds -Force
        $sshConfiguration | Add-Member -NotePropertyName TransferTimeoutSeconds -NotePropertyValue $TransferTimeoutSeconds -Force
        $sshConfiguration | Add-Member -NotePropertyName SshTimeoutSeconds -NotePropertyValue $SshTimeoutSeconds -Force

        if ($BuildDat -or $RebuildDat) {
            $manifestPath = Join-Path $RepoRoot 'assets\dat-manifest.toml'
            $soundfontBootstrap = Join-Path $RepoRoot 'soundfonts\get-fonts.ps1'
            if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
                throw "Missing DAT manifest: $manifestPath"
            }
            if (-not (Test-Path -LiteralPath $soundfontBootstrap -PathType Leaf)) {
                throw "Missing soundfont bootstrap helper: $soundfontBootstrap"
            }
            & $soundfontBootstrap
        }

        $syncResult = Sync-LinuxSourceToVm `
            -RepoRoot $RepoRoot `
            -Vagrant $vagrant `
            -VagrantRoot $LinuxBuildRoot `
            -Environment $environment `
            -Configuration $linuxConfig `
            -MsysPaths $msysPaths `
            -VBoxManage $vboxManage `
            -MachineId $machine.MachineId `
            -WorkRoot $syncWorkRoot `
            -SshConfiguration $sshConfiguration
        Write-Host "Linux source synchronization state: $($syncResult.State); changed items: $($syncResult.ChangedItemCount); log: $($syncResult.LogPath)"

        $profileFolder = if ($Configuration -eq 'Release') { 'release' } else { 'debug' }
        $remoteArtifact = "/home/vagrant/flutzPlayer-cache/target/$profileFolder/flutzplayer"
        $remoteDatPacker = "/home/vagrant/flutzPlayer-cache/target/$profileFolder/flutz_soundfont_tools"
        $cargoArguments = @('build', '--workspace')
        if ($OS -eq "Linux-AppImage") {
            $cargoArguments += '--locked'
        }
        if ($Configuration -eq 'Release') {
            $cargoArguments += '--release'
        }
        if (-not $NoJemallocMemory) {
            $cargoArguments += @('--features', 'flutzplayer/jemalloc-memory')
            Write-Host 'Building Linux binaries with flutzplayer/jemalloc-memory'
        } else {
            Write-Host 'Building Linux binaries without jemalloc for an explicit native-memory comparison'
        }

        if (-not $SkipBuild) {
            $guestSourceLiteral = ConvertTo-LinuxDatShellLiteral -Value $linuxConfig.GuestSourcePath
            $guestBuildCommand = @"
    set -euo pipefail
    export PATH=/home/vagrant/.cargo/bin:`$PATH
    export CARGO_HOME=/home/vagrant/.cargo
    export RUSTUP_HOME=/home/vagrant/.rustup
    cd $guestSourceLiteral
    cargo $($cargoArguments -join ' ')
"@

            Invoke-LinuxGuestCommand `
                -MsysPaths $msysPaths `
                -SshConfiguration $sshConfiguration `
                -Command $guestBuildCommand `
                -Description "Linux $Configuration build with source-static SDL3" | Out-Null

            if ($OS -eq "Linux-AppImage") {
                $allocator = if ($NoJemallocMemory) { 'system' } else { 'jemalloc' }
                Set-LinuxAppImageBuildRecord `
                    -Configuration $linuxConfig `
                    -RemoteBinary $remoteArtifact `
                    -Allocator $allocator `
                    -MsysPaths $msysPaths `
                    -SshConfiguration $sshConfiguration
            }
        }

        if ($BuildDat -or $RebuildDat) {
            Invoke-LinuxDatBuild `
                -RepoRoot $RepoRoot `
                -Configuration $linuxConfig `
                -MsysPaths $msysPaths `
                -SshConfiguration $sshConfiguration
        }

        $hostArtifactRoot = Join-Path $RepoRoot "drops\flutzPlayer-Linux"
        $hostDataRoot = Join-Path $hostArtifactRoot "data"
        $generatedDatRoot = Join-Path $RepoRoot "_local\generated-assets\dat"
        $generatedDatFiles = @()
        if (Test-Path -LiteralPath $generatedDatRoot -PathType Container) {
            $generatedDatFiles = @(
                Get-ChildItem -LiteralPath $generatedDatRoot -Filter '*.dat' -File |
                    Sort-Object Name
            )
        }
        if ($generatedDatFiles.Count -eq 0) {
            throw "Linux runtime DAT assets are required, but none were found at '$generatedDatRoot'. Run .\build-dat.ps1 before building the Linux drop."
        }

        $remoteCheck = @"
    set -euo pipefail
    test -x '$remoteArtifact'
    ldd_output="`$(ldd '$remoteArtifact' 2>&1)" || {
        if ! grep -Fq 'not a dynamic executable' <<<"`$ldd_output"; then
            printf '%s\n' "`$ldd_output" >&2
            exit 1
        fi
    }
    if grep -Fq 'libSDL3.so' <<<"`$ldd_output"; then
        echo 'Unexpected external SDL3 runtime link detected in Linux binary:' >&2
        printf '%s\n' "`$ldd_output" >&2
        exit 1
    fi
    if grep -Fq 'libxkbcommon' <<<"`$ldd_output"; then
        echo 'Unexpected external xkbcommon runtime link detected in Linux binary:' >&2
        printf '%s\n' "`$ldd_output" >&2
        exit 1
    fi
    # Debug panic messages can contain library names even with static linkage.
    nm --defined-only --format=posix '$remoteArtifact' | awk '
        BEGIN {
            split("xkb_context_new xkb_compose_table_new_from_locale xkb_x11_setup_xkb_extension", required)
        }
        `$2 ~ /^[Tt]$/ { defined[`$1] = 1 }
        END {
            for (entry in required) {
                symbol = required[entry]
                if (!(symbol in defined)) {
                    printf "Linux binary is missing static xkbcommon function: %s\n", symbol > "/dev/stderr"
                    failed = 1
                }
            }
            exit failed
        }
    '
    binary_sha256=`$(sha256sum -- '$remoteArtifact' | cut -d ' ' -f 1)
    printf 'REMOTE_BINARY_SHA256 %s\n' "`$binary_sha256"
    printf 'REMOTE_BINARY_PATH %s\n' '$remoteArtifact'
"@
        $remoteArtifactValue = Invoke-LinuxGuestCommand `
            -MsysPaths $msysPaths `
            -SshConfiguration $sshConfiguration `
            -Command $remoteCheck `
            -Description 'Verify source-static SDL3 linkage in Linux binary'
        $binaryHashLine = $remoteArtifactValue.Lines | Where-Object { $_ -cmatch '^REMOTE_BINARY_SHA256 [0-9a-f]{64}$' } | Select-Object -First 1
        $binaryPathLine = $remoteArtifactValue.Lines | Where-Object { $_ -cmatch '^REMOTE_BINARY_PATH /' } | Select-Object -First 1
        if (-not $binaryHashLine -or -not $binaryPathLine) {
            throw "Linux binary was not found at '$remoteArtifact'. Verify the build completed on the VM."
        }
        $remoteArtifactHash = ($binaryHashLine -split ' ', 2)[1]
        $remoteArtifactPath = ($binaryPathLine -split ' ', 2)[1]

        if ($OS -eq "Linux-AppImage") {
            $allocator = if ($NoJemallocMemory) { 'system' } else { 'jemalloc' }
            Invoke-LinuxAppImageBuildAndPublish `
                -RepoRoot $RepoRoot `
                -Configuration $linuxConfig `
                -RemoteBinary $remoteArtifactPath `
                -RemotePacker $remoteDatPacker `
                -Allocator $allocator `
                -MsysPaths $msysPaths `
                -SshConfiguration $sshConfiguration `
                -CleanDrop:$CleanDrop
            return
        }

        $stagingRoot = Join-Path $RepoRoot "_local\linux\build-output\plain-linux-staging-$PID"
        $stagingDataRoot = Join-Path $stagingRoot 'data'
        $stagingPackerRoot = Join-Path $stagingRoot 'dat-packer'
        New-Item -ItemType Directory -Force -Path $stagingDataRoot | Out-Null
        $hostArtifactName = if ($Configuration -eq 'Release') { 'flutzplayer' } else { 'flutzplayer-debug' }
        $stagedArtifactPath = Join-Path $stagingRoot $hostArtifactName
        Copy-LinuxDatGuestFile `
            -MsysPaths $msysPaths `
            -SshConfiguration $sshConfiguration `
            -RemotePath $remoteArtifactPath `
            -Destination $stagedArtifactPath `
            -ExpectedHash $remoteArtifactHash
        Install-LinuxDatPacker `
            -RepoRoot $RepoRoot `
            -RemotePacker $remoteDatPacker `
            -DestinationDirectory $stagingPackerRoot `
            -MsysPaths $msysPaths `
            -SshConfiguration $sshConfiguration
        foreach ($datFile in $generatedDatFiles) {
            $stagedDatPath = Join-Path $stagingDataRoot $datFile.Name
            Copy-Item -LiteralPath $datFile.FullName -Destination $stagedDatPath -Force
            $sourceHash = (Get-FileHash -LiteralPath $datFile.FullName -Algorithm SHA256).Hash
            $stagedHash = (Get-FileHash -LiteralPath $stagedDatPath -Algorithm SHA256).Hash
            if ($sourceHash -ne $stagedHash) {
                throw "Linux DAT staging verification failed for '$($datFile.Name)'."
            }
        }

        if ($CleanDrop) {
            Clear-LinuxPlainDropContents -DropRoot $hostArtifactRoot
        }
        New-Item -ItemType Directory -Force -Path $hostArtifactRoot | Out-Null
        New-Item -ItemType Directory -Force -Path $hostDataRoot | Out-Null
        $hostPackerRoot = Join-Path $hostArtifactRoot 'dat-packer'
        if (Test-Path -LiteralPath $hostPackerRoot -PathType Container) {
            $unexpected = @(Get-ChildItem -LiteralPath $hostPackerRoot -Recurse -Force -File |
                Where-Object { $_.Extension -in @('.exe', '.ps1') })
            if ($unexpected.Count -gt 0) {
                throw "Linux DAT packer directory contains Windows files; remove them or use -CleanDrop: $(($unexpected.FullName) -join ', ')"
            }
        }
        New-Item -ItemType Directory -Force -Path $hostPackerRoot | Out-Null
        $hostArtifactPath = Join-Path $hostArtifactRoot $hostArtifactName
        Copy-Item -LiteralPath $stagedArtifactPath -Destination $hostArtifactPath -Force
        foreach ($packerFile in @('dat-packer', 'pack-dat.sh', 'dat-manifest.toml')) {
            Copy-Item -LiteralPath (Join-Path $stagingPackerRoot $packerFile) -Destination (Join-Path $hostPackerRoot $packerFile) -Force
        }
        Get-ChildItem -LiteralPath $hostDataRoot -Filter '*.dat' -File -ErrorAction SilentlyContinue |
            Remove-Item -Force
        Remove-Item -LiteralPath (Join-Path $hostDataRoot 'preferences.ini') -Force -ErrorAction SilentlyContinue
        foreach ($datFile in $generatedDatFiles) {
            $stagedDatPath = Join-Path $stagingDataRoot $datFile.Name
            $publishedDatPath = Join-Path $hostDataRoot $datFile.Name
            Copy-Item -LiteralPath $stagedDatPath -Destination $publishedDatPath -Force
            $sourceHash = (Get-FileHash -LiteralPath $stagedDatPath -Algorithm SHA256).Hash
            $stagedHash = (Get-FileHash -LiteralPath $publishedDatPath -Algorithm SHA256).Hash
            if ($sourceHash -ne $stagedHash) {
                throw "Linux DAT publication verification failed for '$($datFile.Name)'."
            }
        }

        $publishedBinaryHash = (Get-FileHash -LiteralPath $hostArtifactPath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($publishedBinaryHash -cne $remoteArtifactHash) {
            throw "Linux binary publication checksum mismatch for '$hostArtifactPath'."
        }
        Write-Host "Linux $Configuration build succeeded: $hostArtifactPath; SHA-256 $publishedBinaryHash"
        Write-Host "Runtime data: $hostDataRoot ($($generatedDatFiles.Count) DAT files); DAT tools: $hostPackerRoot"
        Write-Host "Linux build logs: $buildLogRoot"
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
        return
    }
}
