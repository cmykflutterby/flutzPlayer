function ConvertTo-LinuxDatShellLiteral {
    param([Parameter(Mandatory = $true)][string]$Value)

    return "'" + $Value.Replace("'", "'`"'`"'") + "'"
}

function Copy-LinuxDatGuestFile {
    param(
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration,
        [Parameter(Mandatory = $true)][string]$RemotePath,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$ExpectedHash
    )

    $sshConfigPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $SshConfiguration.Path
    $destinationPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $Destination
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
                "$($SshConfiguration.HostAlias):$RemotePath",
                $destinationPath
            ) `
            -Description "Copy verified Linux artifact $RemotePath" `
            -TimeoutSeconds $(if ($SshConfiguration.PSObject.Properties['TransferTimeoutSeconds']) { $SshConfiguration.TransferTimeoutSeconds } else { 1800 }) | Out-Null
    } finally {
        if ($null -eq $previousConversion) {
            Remove-Item Env:MSYS2_ARG_CONV_EXCL -ErrorAction SilentlyContinue
        } else {
            $env:MSYS2_ARG_CONV_EXCL = $previousConversion
        }
    }

    $actualHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash
    if ($actualHash -ne $ExpectedHash) {
        throw "Linux artifact transfer checksum mismatch for '$RemotePath'."
    }
}

function Invoke-LinuxDatBuild {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration
    )

    $guestDirectory = "$($Configuration.GuestArtifactPath.TrimEnd('/'))/generated-dat"
    $guestScript = "$($Configuration.GuestSourcePath.TrimEnd('/'))/build/linux/scripts/build-dat.sh"
    $scriptLiteral = ConvertTo-LinuxDatShellLiteral -Value $guestScript
    $directoryLiteral = ConvertTo-LinuxDatShellLiteral -Value $guestDirectory
    $buildCommand = @"
set -euo pipefail
export PATH=/home/vagrant/.cargo/bin:`$PATH
export CARGO_HOME=/home/vagrant/.cargo
export RUSTUP_HOME=/home/vagrant/.rustup
bash $scriptLiteral $directoryLiteral
"@
    Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command $buildCommand `
        -Description 'Build and validate the Linux DAT family with the release packer' | Out-Null

    $hashCommand = @'
set -euo pipefail
export LC_ALL=C
cd __DAT_DIRECTORY__
shopt -s nullglob
files=(assets-[0-9][0-9][0-9].dat)
((${#files[@]} > 0)) || { echo 'No numbered Linux DAT files were produced' >&2; exit 1; }
for index in "${!files[@]}"; do
    printf -v expected 'assets-%03d.dat' "$index"
    [[ "${files[index]}" == "$expected" && -f "${files[index]}" && ! -L "${files[index]}" ]] || {
        echo "Invalid or noncontiguous Linux DAT part: ${files[index]}" >&2
        exit 1
    }
done
sha256sum -- "${files[@]}"
'@.Replace('__DAT_DIRECTORY__', $directoryLiteral)
    $hashResult = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command $hashCommand `
        -Description 'Hash the validated guest DAT family'

    $files = [System.Collections.Generic.List[object]]::new()
    foreach ($line in $hashResult.Lines) {
        if ($line -notmatch '^([0-9a-f]{64})  (assets-[0-9]{3}\.dat)$') {
            throw "Invalid guest DAT checksum line: $line"
        }
        $expectedName = 'assets-{0:D3}.dat' -f $files.Count
        if ($Matches[2] -cne $expectedName) {
            throw "Guest DAT family is not contiguous: expected '$expectedName', got '$($Matches[2])'."
        }
        $files.Add([pscustomobject]@{ Name = $Matches[2]; Hash = $Matches[1] })
    }
    if ($files.Count -eq 0) {
        throw 'The guest returned no validated DAT checksums.'
    }

    $generatedDirectory = Join-Path $RepoRoot '_local\generated-assets\dat'
    New-Item -ItemType Directory -Force -Path $generatedDirectory | Out-Null
    Get-ChildItem -LiteralPath $generatedDirectory -Force -File | Where-Object {
        $_.Name -ieq 'assets.dat' -or $_.Name -match '^assets-[0-9]+\.dat$'
    } | Remove-Item -Force
    foreach ($file in $files) {
        Copy-LinuxDatGuestFile `
            -MsysPaths $MsysPaths `
            -SshConfiguration $SshConfiguration `
            -RemotePath "$guestDirectory/$($file.Name)" `
            -Destination (Join-Path $generatedDirectory $file.Name) `
            -ExpectedHash $file.Hash
    }
    Write-Host "Validated Linux DAT family copied to $generatedDirectory ($($files.Count) files)."
}

function Install-LinuxDatPacker {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$RemotePacker,
        [Parameter(Mandatory = $true)][string]$DestinationDirectory,
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration
    )

    $wrapper = Join-Path $RepoRoot 'build\linux\scripts\pack-dat.sh'
    $manifest = Join-Path $RepoRoot 'assets\dat-manifest.toml'
    foreach ($source in @($wrapper, $manifest)) {
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "Linux DAT packaging source is missing: $source"
        }
    }
    if (Test-Path -LiteralPath $DestinationDirectory -PathType Container) {
        $unexpected = @(Get-ChildItem -LiteralPath $DestinationDirectory -Recurse -Force -File |
            Where-Object { $_.Extension -in @('.exe', '.ps1') })
        if ($unexpected.Count -gt 0) {
            throw "Linux DAT packer directory contains Windows files; remove them or use -CleanDrop: $(($unexpected.FullName) -join ', ')"
        }
    }

    $packerLiteral = ConvertTo-LinuxDatShellLiteral -Value $RemotePacker
    $checkCommand = @"
set -euo pipefail
packer=$packerLiteral
if [[ ! -f "`$packer" || ! -x "`$packer" ]]; then
    echo "Missing or non-executable Linux DAT packer: `$packer; build without -SkipBuild first" >&2
    exit 1
fi
ldd_output="`$(ldd "`$packer" 2>&1)" || {
    if ! grep -Fq 'not a dynamic executable' <<<"`$ldd_output"; then
        printf '%s\n' "`$ldd_output" >&2
        exit 1
    fi
}
if grep -Fq 'not found' <<<"`$ldd_output"; then
    printf 'Linux DAT packer has missing shared libraries:\n%s\n' "`$ldd_output" >&2
    exit 1
fi
"`$packer" --help >/dev/null
sha256sum -- "`$packer" | cut -d ' ' -f 1
"@
    $hashResult = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -Command $checkCommand `
        -Description 'Validate native Linux DAT packer and dependencies'
    if ($hashResult.Lines.Count -ne 1 -or $hashResult.Lines[0] -notmatch '^[0-9a-f]{64}$') {
        throw 'The guest returned an invalid Linux DAT packer checksum.'
    }

    New-Item -ItemType Directory -Force -Path $DestinationDirectory | Out-Null
    Copy-LinuxDatGuestFile `
        -MsysPaths $MsysPaths `
        -SshConfiguration $SshConfiguration `
        -RemotePath $RemotePacker `
        -Destination (Join-Path $DestinationDirectory 'dat-packer') `
        -ExpectedHash $hashResult.Lines[0]
    Copy-Item -LiteralPath $wrapper -Destination (Join-Path $DestinationDirectory 'pack-dat.sh') -Force
    Copy-Item -LiteralPath $manifest -Destination (Join-Path $DestinationDirectory 'dat-manifest.toml') -Force
    Write-Host "Native Linux DAT packer staged to $DestinationDirectory."
}
