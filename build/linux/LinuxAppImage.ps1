function Set-LinuxAppImageBuildRecord {
    param(
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)][string]$RemoteBinary,
        [Parameter(Mandatory = $true)][string]$Allocator,
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration
    )

    $source = $Configuration.GuestSourcePath.TrimEnd('/')
    $artifact = $Configuration.GuestArtifactPath.TrimEnd('/')
    $script = ConvertTo-LinuxDatShellLiteral -Value "$source/build/linux/scripts/build-appimage.sh"
    $binary = ConvertTo-LinuxDatShellLiteral -Value $RemoteBinary
    $snapshot = ConvertTo-LinuxDatShellLiteral -Value '/home/vagrant/flutzPlayer-staging/source-snapshot.json'
    $record = ConvertTo-LinuxDatShellLiteral -Value "$artifact/appimage-release-build-record.json"
    $allocatorLiteral = ConvertTo-LinuxDatShellLiteral -Value $Allocator
    $command = @"
set -euo pipefail
export PATH=/home/vagrant/.cargo/bin:`$PATH
export CARGO_HOME=/home/vagrant/.cargo
export RUSTUP_HOME=/home/vagrant/.rustup
bash $script record $binary $snapshot $record $allocatorLiteral
"@
    Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command $command `
        -Description 'Record the Linux release binary provenance' | Out-Null
}

function Copy-LinuxAppImageHostFile {
    param(
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration,
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$RemotePath
    )

    $sshConfigPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $SshConfiguration.Path
    $sourcePath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $Source
    $previousConversion = $env:MSYS2_ARG_CONV_EXCL
    try {
        $env:MSYS2_ARG_CONV_EXCL = '*'
        Invoke-LinuxSourceNativeTool `
            -Executable $MsysPaths.Scp `
            -Arguments @(
                '-F', $sshConfigPath,
                '-o', 'BatchMode=yes',
                '-o', "ConnectTimeout=$(if ($SshConfiguration.PSObject.Properties['SshTimeoutSeconds']) { $SshConfiguration.SshTimeoutSeconds } else { 30 })",
                '-o', 'ServerAliveInterval=30',
                '-o', 'ServerAliveCountMax=6',
                $sourcePath,
                "$($SshConfiguration.HostAlias):$RemotePath"
            ) `
            -Description "Copy AppImage DAT part to the guest: $RemotePath" `
            -TimeoutSeconds $(if ($SshConfiguration.PSObject.Properties['TransferTimeoutSeconds']) { $SshConfiguration.TransferTimeoutSeconds } else { 1800 }) | Out-Null
    } finally {
        if ($null -eq $previousConversion) {
            Remove-Item Env:MSYS2_ARG_CONV_EXCL -ErrorAction SilentlyContinue
        } else {
            $env:MSYS2_ARG_CONV_EXCL = $previousConversion
        }
    }
}

function Get-LinuxAppImageDatSources {
    param([Parameter(Mandatory = $true)][string]$Directory)

    if (-not (Test-Path -LiteralPath $Directory -PathType Container)) {
        throw "AppImage DAT cache is missing: $Directory. Run build-dat.ps1 or use -BuildDat."
    }
    $items = @(Get-ChildItem -LiteralPath $Directory -Filter '*.dat' -Force | Sort-Object Name)
    if ($items.Count -eq 0) {
        throw "AppImage DAT cache is empty: $Directory. Run build-dat.ps1 or use -BuildDat."
    }
    for ($index = 0; $index -lt $items.Count; $index++) {
        $item = $items[$index]
        $expectedName = 'assets-{0:D3}.dat' -f $index
        if ($item.Name -cne $expectedName -or $item.Length -le 0) {
            throw "Invalid or noncontiguous AppImage DAT family member '$($item.FullName)'; expected nonempty '$expectedName'."
        }
    }
    return $items
}

function Test-LinuxAppImageGuestData {
    param(
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)][string]$RemoteOutput,
        [Parameter(Mandatory = $true)][string]$RemotePacker,
        [Parameter(Mandatory = $true)][object[]]$Files,
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration
    )

    $remoteData = "$RemoteOutput/data"
    $cache = "$($Configuration.GuestArtifactPath.TrimEnd('/'))/generated-dat"
    $dataLiteral = ConvertTo-LinuxDatShellLiteral -Value $remoteData
    $commands = [System.Collections.Generic.List[string]]::new()
    $commands.Add('set -euo pipefail')
    $commands.Add("mkdir -- $dataLiteral")
    foreach ($file in $Files) {
        $cacheLiteral = ConvertTo-LinuxDatShellLiteral -Value "$cache/$($file.Name)"
        $destinationLiteral = ConvertTo-LinuxDatShellLiteral -Value "$remoteData/$($file.Name)"
        $commands.Add(@"
if [[ -f $cacheLiteral && ! -L $cacheLiteral ]]; then
    cache_hash=`$(sha256sum < $cacheLiteral | cut -d ' ' -f 1)
    if [[ "`$cache_hash" == '$($file.Hash)' ]]; then
        ln -- $cacheLiteral $destinationLiteral
    else
        printf 'UPLOAD %s\n' '$($file.Name)'
    fi
else
    printf 'UPLOAD %s\n' '$($file.Name)'
fi
"@)
    }
    $reuseResult = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command ($commands -join "`n") `
        -Description 'Reuse checksum-matched guest DAT parts when available'
    $requested = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($line in $reuseResult.Lines) {
        if ($line -notmatch '^UPLOAD (assets-[0-9]{3}\.dat)$' -or -not $requested.Add($Matches[1])) {
            throw "Unexpected guest DAT reuse result: $line"
        }
    }
    foreach ($name in $requested) {
        $file = @($Files | Where-Object Name -ceq $name)
        if ($file.Count -ne 1) {
            throw "The guest requested an unknown DAT part: $name"
        }
        Copy-LinuxAppImageHostFile `
            -MsysPaths $MsysPaths `
            -SshConfiguration $SshConfiguration `
            -Source $file[0].Path `
            -RemotePath "$remoteData/$name"
    }

    $checks = [System.Collections.Generic.List[string]]::new()
    foreach ($file in $Files) {
        $part = ConvertTo-LinuxDatShellLiteral -Value "$remoteData/$($file.Name)"
        $checks.Add(@"
[[ -f $part && ! -L $part ]] || { echo 'Missing or invalid AppImage DAT part: $($file.Name)' >&2; exit 1; }
actual=`$(sha256sum < $part | cut -d ' ' -f 1)
[[ "`$actual" == '$($file.Hash)' ]] || { echo 'AppImage DAT checksum mismatch: $($file.Name)' >&2; exit 1; }
"@)
    }
    $packer = ConvertTo-LinuxDatShellLiteral -Value $RemotePacker
    $first = ConvertTo-LinuxDatShellLiteral -Value "$remoteData/assets-000.dat"
    $inspection = ConvertTo-LinuxDatShellLiteral -Value "$RemoteOutput/dat-inspection.json"
    $checkScript = $checks -join "`n"
    $verifyCommand = @"
set -euo pipefail
packer=$packer
[[ -f "`$packer" && -x "`$packer" ]] || { echo 'Missing release DAT packer; build without -SkipBuild first' >&2; exit 1; }
dependencies=`$(ldd "`$packer" 2>&1) || { printf 'Cannot inspect DAT packer dependencies: %s\n' "`$dependencies" >&2; exit 1; }
[[ "`$dependencies" != *'not found'* ]] || { printf '%s\n' "`$dependencies" >&2; exit 1; }
$checkScript
"`$packer" --inspect --input $first --format json > $inspection
jq -e --argjson count $($Files.Count) '.dat_file_count == `$count and .dat_byte_count > 0 and (.soundfonts | type == "array")' $inspection >/dev/null
$checkScript
printf 'APPIMAGE_DAT_VALIDATED %s\n' '$($Files.Count)'
"@
    $validation = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command $verifyCommand `
        -Description 'Inspect and verify the AppImage DAT family'
    $markers = @($validation.Lines | Where-Object { $_ -ceq "APPIMAGE_DAT_VALIDATED $($Files.Count)" })
    if ($markers.Count -ne 1) {
        throw 'The guest did not return a valid AppImage DAT validation marker.'
    }
}

function Invoke-LinuxAppImageBuildAndPublish {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)][string]$RemoteBinary,
        [Parameter(Mandatory = $true)][string]$RemotePacker,
        [Parameter(Mandatory = $true)][string]$Allocator,
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration,
        [switch]$CleanDrop
    )

    $destination = Join-Path $RepoRoot 'drops\flutzPlayer-AppImage'
    $destinationData = Join-Path $destination 'data'
    $destinationPacker = Join-Path $destination 'dat-packer'
    $releaseDirectory = Join-Path $RepoRoot 'drops\flutzPlayer-Linux-Release'
    $logRoot = Join-Path $RepoRoot '_local\linux\build-output'
    New-Item -ItemType Directory -Force -Path $destination, $destinationData, $destinationPacker, $releaseDirectory, $logRoot | Out-Null
    if ($CleanDrop) {
        Get-ChildItem -LiteralPath $destination -Force | Where-Object Name -notin @('data', 'dat-packer') | Remove-Item -Recurse -Force
        Get-ChildItem -LiteralPath $destinationData, $destinationPacker, $releaseDirectory -Force | Remove-Item -Recurse -Force
    }

    $sources = @(Get-LinuxAppImageDatSources -Directory (Join-Path $RepoRoot '_local\generated-assets\dat'))
    $guestSource = $Configuration.GuestSourcePath.TrimEnd('/')
    $guestArtifact = $Configuration.GuestArtifactPath.TrimEnd('/')
    $remoteOutput = "$guestArtifact/appimage"
    $script = ConvertTo-LinuxDatShellLiteral -Value "$guestSource/build/linux/scripts/build-appimage.sh"
    $binary = ConvertTo-LinuxDatShellLiteral -Value $RemoteBinary
    $snapshot = ConvertTo-LinuxDatShellLiteral -Value '/home/vagrant/flutzPlayer-staging/source-snapshot.json'
    $record = ConvertTo-LinuxDatShellLiteral -Value "$guestArtifact/appimage-release-build-record.json"
    $output = ConvertTo-LinuxDatShellLiteral -Value $remoteOutput
    $allocatorLiteral = ConvertTo-LinuxDatShellLiteral -Value $Allocator
    $buildCommand = @"
set -euo pipefail
export PATH=/home/vagrant/.cargo/bin:`$PATH
export CARGO_HOME=/home/vagrant/.cargo
export RUSTUP_HOME=/home/vagrant/.rustup
bash $script build $binary $snapshot $record $output $allocatorLiteral
"@
    $logPath = Join-Path $logRoot 'appimage.log'
    $result = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command $buildCommand `
        -Description 'Assemble and validate the Linux AppImage' `
        -LogPath $logPath
    $markers = @($result.Lines | Where-Object {
        $_ -cmatch '^APPIMAGE_RESULT [0-9a-f]{64} [0-9a-f]{64}$'
    })
    if ($markers.Count -ne 1 -or $markers[0] -cnotmatch '^APPIMAGE_RESULT ([0-9a-f]{64}) ([0-9a-f]{64})$') {
        throw "The guest did not return valid AppImage and metadata checksums. See '$logPath'."
    }
    $imageHash = $Matches[1]
    $metadataHash = $Matches[2]
    $metadataPath = Join-Path $logRoot 'appimage.metadata.json'
    Copy-LinuxDatGuestFile `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -RemotePath "$remoteOutput/metadata.json" `
        -Destination $metadataPath `
        -ExpectedHash $metadataHash
    $metadata = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json
    $pins = Get-Content -LiteralPath (Join-Path $RepoRoot 'build\linux\tool-versions.json') -Raw | ConvertFrom-Json
    $hostLockHash = (Get-FileHash -LiteralPath (Join-Path $RepoRoot 'Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($metadata.schemaId -cne 'https://flutzplayer.local/schemas/appimage-build-metadata/v1' -or
        $metadata.appimage.name -cne 'flutzPlayer-x86_64.AppImage' -or
        $metadata.appimage.sha256 -cne $imageHash -or
        $metadata.build.sourceSnapshotSha256 -cnotmatch '^[0-9a-f]{64}$' -or
        $metadata.build.cargoLockSha256 -cne $hostLockHash -or
        $metadata.tools.linuxdeploy.sha256 -cne $pins.linuxdeploy.sha256 -or
        $metadata.tools.appimagetool.sha256 -cne $pins.appimagetool.sha256 -or
        $metadata.tools.runtime.sha256 -cne $pins.appimageRuntime.sha256 -or
        $metadata.tools.runtime.commit -cne $pins.appimageRuntime.commit) {
        throw "AppImage build metadata is invalid: $metadataPath"
    }

    $files = [System.Collections.Generic.List[object]]::new()
    foreach ($source in $sources) {
        $files.Add([pscustomobject]@{
            Name = $source.Name
            Path = $source.FullName
            Hash = (Get-FileHash -LiteralPath $source.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        })
    }
    Test-LinuxAppImageGuestData `
        -Configuration $Configuration `
        -RemoteOutput $remoteOutput `
        -RemotePacker $RemotePacker `
        -Files @($files) `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration

    $packageScript = ConvertTo-LinuxDatShellLiteral -Value "$guestSource/build/linux/scripts/package-drop.sh"
    $packer = ConvertTo-LinuxDatShellLiteral -Value $RemotePacker
    $packageLogPath = Join-Path $logRoot 'release-package.log'
    $package = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command "set -euo pipefail`nbash $packageScript $output $packer" `
        -Description 'Create and validate the Linux release archive' `
        -LogPath $packageLogPath
    $releaseFiles = [System.Collections.Generic.List[object]]::new()
    $archiveMarkers = [System.Collections.Generic.List[object]]::new()
    foreach ($line in $package.Lines) {
        if ($line -cmatch '^RELEASE_FILE ([0-9a-f]{64}) ((?:data/assets-[0-9]{3}\.dat)|flutzPlayer-x86_64\.AppImage|dat-packer/(?:dat-packer|pack-dat\.sh|dat-manifest\.toml))$') {
            $releaseFiles.Add([pscustomobject]@{ Hash = $Matches[1]; Path = $Matches[2] })
        } elseif ($line -cmatch '^RELEASE_ARCHIVE ([0-9a-f]{64}) ([0-9a-f]{64})$') {
            $archiveMarkers.Add([pscustomobject]@{ Archive = $Matches[1]; Checksum = $Matches[2] })
        } elseif ($line -cmatch '^RELEASE_') {
            throw "Unexpected guest release marker: $line"
        }
    }
    $expectedPaths = @('dat-packer/dat-manifest.toml', 'dat-packer/dat-packer', 'dat-packer/pack-dat.sh', 'flutzPlayer-x86_64.AppImage') +
        @($files | ForEach-Object { "data/$($_.Name)" })
    $reportedPaths = @($releaseFiles | ForEach-Object Path)
    if ($archiveMarkers.Count -ne 1 -or
        (Compare-Object -ReferenceObject @($expectedPaths | Sort-Object) -DifferenceObject @($reportedPaths | Sort-Object) -CaseSensitive) -or
        @($releaseFiles | Where-Object Path -ceq 'flutzPlayer-x86_64.AppImage')[0].Hash -cne $imageHash) {
        throw "The guest did not return a valid release archive file list. See '$packageLogPath'."
    }
    foreach ($file in $files) {
        if (@($releaseFiles | Where-Object Path -ceq "data/$($file.Name)")[0].Hash -cne $file.Hash) {
            throw "The release archive DAT part differs from the validated cache: $($file.Name)"
        }
    }

    $archiveName = 'flutzPlayer-linux-x86_64.tar.gz'
    $checksumName = "$archiveName.sha256.txt"
    $archivePath = Join-Path $releaseDirectory $archiveName
    $checksumPath = Join-Path $releaseDirectory $checksumName
    Copy-LinuxDatGuestFile `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -RemotePath "$remoteOutput/release/$archiveName" `
        -Destination $archivePath `
        -ExpectedHash $archiveMarkers[0].Archive
    Copy-LinuxDatGuestFile `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -RemotePath "$remoteOutput/release/$checksumName" `
        -Destination $checksumPath `
        -ExpectedHash $archiveMarkers[0].Checksum
    if ([System.IO.File]::ReadAllText($checksumPath) -cne "$($archiveMarkers[0].Archive)  $archiveName`n") {
        throw "Release checksum file has unexpected contents: $checksumPath"
    }

    # The drop must hold exactly the archive contents; only runtime preferences survive, and the
    # drop, data, and dat-packer directories are kept to preserve their Dropbox identity.
    Get-ChildItem -LiteralPath $destination -Force | Where-Object Name -notin @('data', 'dat-packer') | Remove-Item -Recurse -Force
    Get-ChildItem -LiteralPath $destinationData -Force | Where-Object Name -ne 'preferences.ini' | Remove-Item -Recurse -Force
    Get-ChildItem -LiteralPath $destinationPacker -Force | Remove-Item -Recurse -Force
    $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    & $tar -x -z -f $archivePath -C $destination --strip-components 1
    if ($LASTEXITCODE -ne 0) {
        throw "Extracting the release archive into '$destination' failed with exit code $LASTEXITCODE."
    }
    foreach ($releaseFile in $releaseFiles) {
        $path = Join-Path $destination ($releaseFile.Path.Replace('/', '\'))
        if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or
            (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $releaseFile.Hash) {
            throw "AppImage drop file does not match the release archive: $path"
        }
    }
    $destinationRoot = (Resolve-Path -LiteralPath $destination).Path.TrimEnd('\')
    $dropPaths = @(Get-ChildItem -LiteralPath $destinationRoot -Recurse -Force -File | ForEach-Object {
        $_.FullName.Substring($destinationRoot.Length + 1).Replace('\', '/')
    } | Where-Object { $_ -cne 'data/preferences.ini' })
    if (Compare-Object -ReferenceObject @($reportedPaths | Sort-Object) -DifferenceObject @($dropPaths | Sort-Object) -CaseSensitive) {
        throw "AppImage drop contents differ from the release archive: $destination"
    }
    Write-Host "Linux release archive and checksum published to $releaseDirectory (SHA-256 $($archiveMarkers[0].Archive))."
    Write-Host "AppImage drop populated from the release archive: $destination ($($releaseFiles.Count) files, $($files.Count) DAT parts)."
    Write-Host "AppImage build log: $logPath"
    Write-Host "Release packaging log: $packageLogPath"
    Write-Host "AppImage build metadata: $metadataPath"
    Write-Host "Guest validation output: $remoteOutput"
}
