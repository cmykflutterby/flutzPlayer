$script:LinuxSourceSyncRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

function Invoke-LinuxSourceNativeTool {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Description,
        [string]$LogPath,
        [ValidateRange(1, 86400)][int]$TimeoutSeconds = 1800
    )

    $output = Invoke-DevelopmentVmTool `
        -Executable $Executable `
        -Arguments $Arguments `
        -Description $Description `
        -LogPath $LogPath `
        -TimeoutSeconds $TimeoutSeconds
    [string[]]$lines = if ([string]::IsNullOrWhiteSpace($output)) { @() } else { @($output -split "`r?`n") }
    [pscustomobject]@{
        ExitCode = 0
        Lines = $lines
    }
}

function ConvertTo-MsysPath {
    param(
        [Parameter(Mandatory = $true)][string]$Cygpath,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $result = Invoke-LinuxSourceNativeTool `
        -Executable $Cygpath `
        -Arguments @('-u', $Path) `
        -Description "MSYS2 path conversion for '$Path'"
    if ($result.Lines.Count -ne 1 -or [string]::IsNullOrWhiteSpace($result.Lines[0])) {
        throw "MSYS2 path conversion returned an invalid result for '$Path'."
    }
    $result.Lines[0]
}

function Test-IPv4AddressInCidr {
    param(
        [Parameter(Mandatory = $true)][string]$Address,
        [Parameter(Mandatory = $true)][string]$Cidr
    )

    $parts = $Cidr.Split('/')
    if ($parts.Count -ne 2) {
        return $false
    }
    $addressValue = $null
    $networkValue = $null
    $prefixLength = 0
    if (
        -not [System.Net.IPAddress]::TryParse($Address, [ref]$addressValue) -or
        -not [System.Net.IPAddress]::TryParse($parts[0], [ref]$networkValue) -or
        -not [int]::TryParse($parts[1], [ref]$prefixLength) -or
        $prefixLength -lt 0 -or
        $prefixLength -gt 32
    ) {
        return $false
    }
    $addressBytes = $addressValue.GetAddressBytes()
    $networkBytes = $networkValue.GetAddressBytes()
    if ($addressBytes.Count -ne 4 -or $networkBytes.Count -ne 4) {
        return $false
    }
    for ($index = 0; $index -lt 4; $index++) {
        $bits = [Math]::Max(0, [Math]::Min(8, $prefixLength - ($index * 8)))
        $mask = if ($bits -eq 0) { 0 } else { 256 - [Math]::Pow(2, 8 - $bits) }
        if (($addressBytes[$index] -band $mask) -ne ($networkBytes[$index] -band $mask)) {
            return $false
        }
    }
    $true
}

function Get-DevelopmentVmGuestIPv4Addresses {
    param(
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$MachineId,
        [Parameter(Mandatory = $true)][string]$TrustedCidr
    )

    $properties = Invoke-DevelopmentVmTool `
        -Executable $VBoxManage `
        -Arguments @('guestproperty', 'enumerate', $MachineId) `
        -Description 'Development VM live network address discovery' `
        -TimeoutSeconds 60
    @(
        [regex]::Matches(
            $properties,
            "(?m)^/VirtualBox/GuestInfo/Net/\d+/V4/IP\s+=\s+'(\d+\.\d+\.\d+\.\d+)'"
        ) |
            ForEach-Object { $_.Groups[1].Value } |
            Where-Object {
                $_ -ne '127.0.0.1' -and
                $_ -notlike '169.254.*' -and
                (Test-IPv4AddressInCidr -Address $_ -Cidr $TrustedCidr)
            } |
            Select-Object -Unique
    )
}

function Get-LinuxVagrantSshConfiguration {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][hashtable]$Environment,
        [Parameter(Mandatory = $true)][string]$OutputPath
    )

    $raw = Invoke-DevelopmentVmTool `
        -Executable $Vagrant `
        -Arguments @('ssh-config') `
        -Description 'Vagrant SSH configuration discovery' `
        -Environment $Environment `
        -WorkingDirectory $VagrantRoot `
        -TimeoutSeconds 120
    $hostMatch = [regex]::Match($raw, '(?m)^Host\s+(\S+)\s*$')
    if (-not $hostMatch.Success) {
        throw 'Vagrant SSH configuration did not contain a Host alias.'
    }
    foreach ($required in @('HostName', 'User', 'Port', 'IdentityFile')) {
        if ($raw -notmatch "(?m)^\s+$required\s+\S") {
            throw "Vagrant SSH configuration did not contain required field '$required'."
        }
    }
    $portMatch = [regex]::Match($raw, '(?m)^\s+Port\s+(\d+)\s*$')
    if (-not $portMatch.Success -or [int]$portMatch.Groups[1].Value -lt 1 -or [int]$portMatch.Groups[1].Value -gt 65535) {
        throw 'Vagrant SSH configuration contained an invalid port.'
    }

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $OutputPath) | Out-Null
    $normalized = $raw.Replace("`r`n", "`n").TrimEnd() + "`n"
    [System.IO.File]::WriteAllText($OutputPath, $normalized, [System.Text.UTF8Encoding]::new($false))
    $hostNameMatch = [regex]::Match($normalized, '(?m)^\s+HostName\s+(\S+)\s*$')
    [pscustomobject]@{
        HostAlias = $hostMatch.Groups[1].Value
        HostName = $hostNameMatch.Groups[1].Value
        Port = [int]$portMatch.Groups[1].Value
        Path = $OutputPath
        Raw = $normalized
    }
}

function Test-LinuxSshConfiguration {
    param(
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration,
        [Parameter(Mandatory = $true)][string]$ExpectedHostname,
        [Parameter(Mandatory = $true)][string]$ExpectedTesterUsername,
        [ValidateRange(5, 300)][int]$ConnectTimeoutSeconds = 30
    )

    $configPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $SshConfiguration.Path
    if (
        $ExpectedHostname -notmatch '^[a-zA-Z0-9][a-zA-Z0-9.-]*$' -or
        $ExpectedTesterUsername -notmatch '^[a-z_][a-z0-9_-]{2,31}$'
    ) {
        throw 'Expected development VM hostname or tester username is invalid.'
    }
    $identityProbe = "test `"`$(hostname)`" = '$ExpectedHostname' && test `"`$(cat /etc/flutzplayer-build/rdp-test-user 2>/dev/null)`" = '$ExpectedTesterUsername' && id '$ExpectedTesterUsername' >/dev/null 2>&1"
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $MsysPaths.Ssh `
            -F $configPath `
            -o BatchMode=yes `
            -o "ConnectTimeout=$ConnectTimeoutSeconds" `
            -o ServerAliveInterval=30 `
            -o ServerAliveCountMax=6 `
            -o ConnectionAttempts=1 `
            $SshConfiguration.HostAlias `
            $identityProbe 2>&1 | Out-Null
        $LASTEXITCODE -eq 0
    } finally {
        $ErrorActionPreference = $previousPreference
    }
}

function Resolve-LinuxSshConfiguration {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][hashtable]$Environment,
        [Parameter(Mandatory = $true)][string]$OutputPath,
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$MachineId,
        [Parameter(Mandatory = $true)][string]$TrustedCidr,
        [ValidateRange(5, 300)][int]$ConnectTimeoutSeconds = 30
    )

    $configuration = Get-LinuxVagrantSshConfiguration `
        -Vagrant $Vagrant `
        -VagrantRoot $VagrantRoot `
        -Environment $Environment `
        -OutputPath $OutputPath
    $credentialPath = [string]$Environment.FLUTZ_LINUX_RDP_CREDENTIAL_FILE
    if (-not (Test-Path -LiteralPath $credentialPath -PathType Leaf)) {
        throw "Development VM tester credential file is missing: $credentialPath"
    }
    $testerUsername = [string](Get-Content -LiteralPath $credentialPath -Raw | ConvertFrom-Json).username
    $expectedHostname = [string]$Environment.FLUTZ_LINUX_GUEST_HOSTNAME
    if (
        Test-LinuxSshConfiguration `
            -MsysPaths $MsysPaths `
            -SshConfiguration $configuration `
            -ExpectedHostname $expectedHostname `
            -ExpectedTesterUsername $testerUsername `
            -ConnectTimeoutSeconds $ConnectTimeoutSeconds
    ) {
        Write-Host "Vagrant-derived SSH endpoint matches development VM '$expectedHostname'."
        return $configuration
    }

    Write-Host 'Vagrant-derived SSH endpoint is not reachable; discovering current guest LAN addresses from VirtualBox Guest Additions.'
    $addresses = @(
        Get-DevelopmentVmGuestIPv4Addresses `
            -VBoxManage $VBoxManage `
            -MachineId $MachineId `
            -TrustedCidr $TrustedCidr
    )
    foreach ($address in $addresses) {
        $candidate = [regex]::Replace(
            $configuration.Raw,
            '(?m)^(\s+HostName\s+)\S+\s*$',
            ('${1}' + $address)
        )
        $candidate = [regex]::Replace(
            $candidate,
            '(?m)^(\s+Port\s+)\d+\s*$',
            '${1}22'
        )
        [System.IO.File]::WriteAllText(
            $OutputPath,
            $candidate,
            [System.Text.UTF8Encoding]::new($false)
        )
        $candidateConfiguration = [pscustomobject]@{
            HostAlias = $configuration.HostAlias
            HostName = $address
            Port = 22
            Path = $OutputPath
            Raw = $candidate
        }
        if (
            Test-LinuxSshConfiguration `
                -MsysPaths $MsysPaths `
                -SshConfiguration $candidateConfiguration `
                -ExpectedHostname $expectedHostname `
                -ExpectedTesterUsername $testerUsername `
                -ConnectTimeoutSeconds $ConnectTimeoutSeconds
        ) {
            Write-Host "Using current LAN address $address for verified development VM '$expectedHostname'."
            return $candidateConfiguration
        }
    }

    $addressSummary = if ($addresses.Count) { $addresses -join ', ' } else { 'none' }
    throw "SSH connectivity to development VM '$MachineId' failed. Vagrant's endpoint was unreachable, and no reachable Guest Additions address in trusted CIDR '$TrustedCidr' was found. Candidates: $addressSummary."
}

function Invoke-LinuxGuestCommand {
    param(
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$Description,
        [string]$LogPath
    )

    $commandTimeout = if ($SshConfiguration.PSObject.Properties['CommandTimeoutSeconds']) {
        [int]$SshConfiguration.CommandTimeoutSeconds
    } else {
        14400
    }
    if ([string]::IsNullOrWhiteSpace($LogPath)) {
        $safeDescription = [regex]::Replace($Description, '[^A-Za-z0-9._-]+', '-').Trim('-')
        $logRoot = [System.IO.Path]::GetFullPath(
            (Join-Path $script:LinuxSourceSyncRoot '..\..\_local\linux\build-output')
        )
        $LogPath = Join-Path $logRoot ("guest-{0}-{1}.log" -f [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff'), $safeDescription)
    }
    $connectTimeout = if ($SshConfiguration.PSObject.Properties['SshTimeoutSeconds']) {
        [int]$SshConfiguration.SshTimeoutSeconds
    } else {
        30
    }
    $configPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $SshConfiguration.Path
    $normalizedCommand = $Command.Replace("`r`n", "`n")
    Invoke-LinuxSourceNativeTool `
        -Executable $MsysPaths.Ssh `
        -Arguments @(
            '-F', $configPath,
            '-o', 'BatchMode=yes',
            '-o', "ConnectTimeout=$connectTimeout",
            '-o', 'ServerAliveInterval=30',
            '-o', 'ServerAliveCountMax=6',
            $SshConfiguration.HostAlias,
            $normalizedCommand
        ) `
        -Description $Description `
        -LogPath $LogPath `
        -TimeoutSeconds $commandTimeout
}

function Sync-LinuxSourceToVm {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][hashtable]$Environment,
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$MachineId,
        [Parameter(Mandatory = $true)][string]$WorkRoot,
        $SshConfiguration,
        [switch]$DryRun
    )

    $excludePath = Join-Path $script:LinuxSourceSyncRoot 'source-sync.exclude'
    if (-not (Test-Path -LiteralPath $excludePath -PathType Leaf)) {
        throw "Linux source-sync exclusion file is missing: $excludePath"
    }
    foreach ($tool in @($MsysPaths.Cygpath, $MsysPaths.Rsync, $MsysPaths.Ssh, $MsysPaths.Scp)) {
        if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) {
            throw "Required MSYS2 synchronization tool is missing: $tool"
        }
    }

    if ($null -eq $SshConfiguration) {
        $SshConfiguration = Resolve-LinuxSshConfiguration `
            -Vagrant $Vagrant `
            -VagrantRoot $VagrantRoot `
            -Environment $Environment `
            -OutputPath (Join-Path $WorkRoot 'ssh-config') `
            -MsysPaths $MsysPaths `
            -VBoxManage $VBoxManage `
            -MachineId $MachineId `
            -TrustedCidr $Configuration.RdpSourceCidr
    }
    $sshConfiguration = $SshConfiguration
    $sshTimeout = if ($sshConfiguration.PSObject.Properties['SshTimeoutSeconds']) {
        [int]$sshConfiguration.SshTimeoutSeconds
    } else {
        30
    }
    $transferTimeout = if ($sshConfiguration.PSObject.Properties['TransferTimeoutSeconds']) {
        [int]$sshConfiguration.TransferTimeoutSeconds
    } else {
        1800
    }

    $repoPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $RepoRoot
    $excludeMsysPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $excludePath
    $sshConfigMsysPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $sshConfiguration.Path
    $guestSourcePath = [string]$Configuration.GuestSourcePath
    $guestCachePath = '/home/vagrant/flutzPlayer-cache'
    $snapshotPath = '/home/vagrant/flutzPlayer-staging/source-snapshot.json'

    $guestSourceLiteral = ConvertTo-LinuxDatShellLiteral -Value $guestSourcePath
    $guestCacheLiteral = ConvertTo-LinuxDatShellLiteral -Value $guestCachePath
    $snapshotParent = (Split-Path -Parent $snapshotPath).Replace('\', '/')
    $snapshotDirectoryLiteral = ConvertTo-LinuxDatShellLiteral -Value $snapshotParent
    Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $sshConfiguration `
        -Command "mkdir -p -- $guestSourceLiteral $guestCacheLiteral $snapshotDirectoryLiteral" `
        -Description 'Guest source staging preparation' | Out-Null
    if (-not $DryRun) {
        $guestCleanupCommand = @"
set -euo pipefail
source=$guestSourceLiteral
rm -rf -- "`$source/.git" "`$source/.vagrant" "`$source/_local" "`$source/drops" "`$source/design-docs"
find "`$source" -type d \( -name target -o -name AppDir \) -prune -exec rm -rf -- {} +
find "`$source" -type f \( -iname '*.exe' -o -iname '*.dll' -o -iname '*.pdb' -o -iname '*.lib' -o -iname '*.exp' -o -iname '*.ilk' -o -iname '*.AppImage' -o -iname '*.AppImage.zsync' \) -delete
"@
        Invoke-LinuxGuestCommand `
            -MsysPaths $MsysPaths `
            -SshConfiguration $sshConfiguration `
            -Command $guestCleanupCommand `
            -Description 'Guest prohibited host-artifact cleanup' | Out-Null
    }

    $timestamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')
    $logPath = Join-Path $WorkRoot "logs\rsync-$timestamp.log"
    $arguments = @(
        '--archive',
        '--delete',
        '--omit-dir-times',
        '--itemize-changes',
        '--human-readable',
        '--safe-links',
        '--chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r',
        "--exclude-from=$excludeMsysPath",
        "--rsh=/usr/bin/ssh -F $(ConvertTo-LinuxDatShellLiteral -Value $sshConfigMsysPath) -o BatchMode=yes -o ConnectTimeout=$sshTimeout -o ServerAliveInterval=30 -o ServerAliveCountMax=6",
        '--protect-args'
    )
    if ($DryRun) {
        $arguments += '--dry-run'
    }
    $arguments += @('--', "$repoPath/", "$($sshConfiguration.HostAlias):$guestSourcePath/")
    $previousArgConversion = $env:MSYS2_ARG_CONV_EXCL
    try {
        $env:MSYS2_ARG_CONV_EXCL = '*'
        $syncResult = Invoke-LinuxSourceNativeTool `
            -Executable $MsysPaths.Rsync `
            -Arguments $arguments `
            -Description $(if ($DryRun) { 'Linux source rsync dry run' } else { 'Linux source rsync' }) `
            -LogPath $logPath `
            -TimeoutSeconds $transferTimeout
    } finally {
        $env:MSYS2_ARG_CONV_EXCL = $previousArgConversion
    }
    $changedLines = @($syncResult.Lines | Where-Object {
        $_ -match '^[<>ch\.\*][fdLDS]' -or $_ -match '^\*deleting\s'
    })

    if ($DryRun) {
        return [pscustomobject]@{
            State = 'dry-run'
            ChangedItemCount = $changedLines.Count
            LogPath = $logPath
            SshConfigPath = $sshConfiguration.Path
            SnapshotPath = $snapshotPath
        }
    }

    $hydrateScript = "$guestSourcePath/build/linux/sync/hydrate-linux-source.sh"
    $snapshotScript = "$guestSourcePath/build/linux/sync/write-source-snapshot.sh"
    $hydrateScriptLiteral = ConvertTo-LinuxDatShellLiteral -Value $hydrateScript
    $snapshotScriptLiteral = ConvertTo-LinuxDatShellLiteral -Value $snapshotScript
    $snapshotPathLiteral = ConvertTo-LinuxDatShellLiteral -Value $snapshotPath
    $hydrate = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $sshConfiguration `
        -Command "bash $hydrateScriptLiteral $guestSourceLiteral $guestCacheLiteral" `
        -Description 'Linux guest vendor hydration'
    if (($hydrate.Lines -join "`n") -notmatch 'LINUX_VENDOR_OK') {
        throw 'Linux guest vendor hydration completed without its success marker.'
    }
    $snapshot = Invoke-LinuxGuestCommand `
        -MsysPaths $MsysPaths `
        -SshConfiguration $sshConfiguration `
        -Command "bash $snapshotScriptLiteral $guestSourceLiteral $snapshotPathLiteral" `
        -Description 'Linux guest source snapshot generation'
    $snapshotMatch = [regex]::Match(($snapshot.Lines -join "`n"), 'SOURCE_SNAPSHOT_OK entries=(\d+) sha256=([0-9a-f]{64})')
    if (-not $snapshotMatch.Success) {
        throw 'Linux guest snapshot generation completed without a valid success marker.'
    }

    Write-Host "Linux source sync summary: changedItems=$($changedLines.Count), exclusions='$excludePath', snapshot=$($snapshotMatch.Groups[2].Value)"
    [pscustomobject]@{
        State = if ($changedLines.Count -eq 0) { 'ready' } else { 'synchronized' }
        ChangedItemCount = $changedLines.Count
        LogPath = $logPath
        SshConfigPath = $sshConfiguration.Path
        SnapshotPath = $snapshotPath
        SnapshotEntryCount = [int]$snapshotMatch.Groups[1].Value
        SnapshotSha256 = $snapshotMatch.Groups[2].Value
    }
}

function Receive-LinuxGuestFile {
    param(
        [Parameter(Mandatory = $true)]$MsysPaths,
        [Parameter(Mandatory = $true)]$SshConfiguration,
        [Parameter(Mandatory = $true)][string]$GuestPath,
        [Parameter(Mandatory = $true)][string]$HostPath
    )

    if ([string]::IsNullOrWhiteSpace($GuestPath) -or $GuestPath.EndsWith('/')) {
        throw 'SCP retrieval requires one explicit guest file path, not a directory.'
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $HostPath) | Out-Null
    $configPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $SshConfiguration.Path
    $hostMsysPath = ConvertTo-MsysPath -Cygpath $MsysPaths.Cygpath -Path $HostPath
    Invoke-LinuxSourceNativeTool `
        -Executable $MsysPaths.Scp `
        -Arguments @(
            '-F', $configPath,
            '-o', 'BatchMode=yes',
            '-o', "ConnectTimeout=$(if ($SshConfiguration.PSObject.Properties['SshTimeoutSeconds']) { $SshConfiguration.SshTimeoutSeconds } else { 30 })",
            '-o', 'ServerAliveInterval=30',
            '-o', 'ServerAliveCountMax=6',
            "$($SshConfiguration.HostAlias):$GuestPath",
            $hostMsysPath
        ) `
        -Description "SCP retrieval of '$GuestPath'" `
        -TimeoutSeconds $(if ($SshConfiguration.PSObject.Properties['TransferTimeoutSeconds']) { $SshConfiguration.TransferTimeoutSeconds } else { 1800 }) | Out-Null
    if (-not (Test-Path -LiteralPath $HostPath -PathType Leaf)) {
        throw "SCP reported success but did not create '$HostPath'."
    }
    Get-Item -LiteralPath $HostPath
}
