$script:LinuxDevelopmentVmRoot = $PSScriptRoot

function ConvertTo-WindowsProcessArgument {
    param([AllowEmptyString()][string]$Value)

    if ($Value -notmatch '[\s"]' -and $Value.Length -gt 0) {
        return $Value
    }
    $result = [System.Text.StringBuilder]::new()
    [void]$result.Append('"')
    $backslashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') {
            $backslashes++
        } elseif ($character -eq '"') {
            [void]$result.Append(('\' * ($backslashes * 2 + 1)))
            [void]$result.Append('"')
            $backslashes = 0
        } else {
            if ($backslashes) { [void]$result.Append(('\' * $backslashes)) }
            [void]$result.Append($character)
            $backslashes = 0
        }
    }
    if ($backslashes) { [void]$result.Append(('\' * ($backslashes * 2))) }
    [void]$result.Append('"')
    $result.ToString()
}

function Invoke-DevelopmentVmTool {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Description,
        [hashtable]$Environment = @{},
        [string]$WorkingDirectory = (Get-Location).ProviderPath,
        [string]$LogPath,
        [ValidateRange(1, 86400)][int]$TimeoutSeconds = 300
    )

    $start = [DateTime]::UtcNow
    Write-Host "[$($start.ToString('o'))] Starting: $Description"
    $logWriter = $null
    if (-not [string]::IsNullOrWhiteSpace($LogPath)) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogPath) | Out-Null
        $logWriter = [System.IO.StreamWriter]::new($LogPath, $false, [System.Text.UTF8Encoding]::new($false))
        $logWriter.AutoFlush = $true
        $logWriter.WriteLine("[$($start.ToString('o'))] Starting: $Description")
        Write-Host "Live log: $LogPath"
    }

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.CreateNoWindow = $true
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.Arguments = ($Arguments | ForEach-Object {
        ConvertTo-WindowsProcessArgument -Value $_
    }) -join ' '
    foreach ($entry in $Environment.GetEnumerator()) {
        $startInfo.Environment[$entry.Key] = [string]$entry.Value
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $output = [System.Text.StringBuilder]::new()
    try {
        if (-not $process.Start()) {
            throw "Failed to start $Description."
        }
        $stdoutTask = $process.StandardOutput.ReadLineAsync()
        $stderrTask = $process.StandardError.ReadLineAsync()
        $stdoutOpen = $true
        $stderrOpen = $true
        $lastProgress = [DateTime]::UtcNow
        while (-not $process.HasExited -or $stdoutOpen -or $stderrOpen) {
            while ($stdoutOpen -and $stdoutTask.IsCompleted) {
                $line = $stdoutTask.GetAwaiter().GetResult()
                if ($null -eq $line) {
                    $stdoutOpen = $false
                } else {
                    Write-Host $line
                    if ($logWriter) { $logWriter.WriteLine($line) }
                    [void]$output.AppendLine($line)
                    $stdoutTask = $process.StandardOutput.ReadLineAsync()
                }
            }
            while ($stderrOpen -and $stderrTask.IsCompleted) {
                $line = $stderrTask.GetAwaiter().GetResult()
                if ($null -eq $line) {
                    $stderrOpen = $false
                } else {
                    Write-Host $line
                    if ($logWriter) { $logWriter.WriteLine($line) }
                    [void]$output.AppendLine($line)
                    $stderrTask = $process.StandardError.ReadLineAsync()
                }
            }

            $now = [DateTime]::UtcNow
            if (($now - $start).TotalSeconds -ge $TimeoutSeconds -and -not $process.HasExited) {
                $process.Kill($true)
                $process.WaitForExit()
                throw "$Description timed out after $TimeoutSeconds seconds."
            }
            if (($now - $lastProgress).TotalSeconds -ge 30 -and -not $process.HasExited) {
                $progress = "[$($now.ToString('o'))] Still running ($([Math]::Round(($now - $start).TotalMinutes, 1)) min): $Description"
                Write-Host $progress
                if ($logWriter) { $logWriter.WriteLine($progress) }
                $lastProgress = $now
            }
            if (-not $process.HasExited -or $stdoutOpen -or $stderrOpen) {
                Start-Sleep -Milliseconds 50
            }
        }
        $process.WaitForExit()
        $elapsed = [Math]::Round(([DateTime]::UtcNow - $start).TotalSeconds, 1)
        if ($process.ExitCode -ne 0) {
            throw "$Description failed with exit code $($process.ExitCode)."
        }
        $complete = "[$([DateTime]::UtcNow.ToString('o'))] Completed in ${elapsed}s: $Description"
        Write-Host $complete
        if ($logWriter) { $logWriter.WriteLine($complete) }
        $output.ToString().Trim()
    } finally {
        if ($logWriter) { $logWriter.Dispose() }
        $process.Dispose()
    }
}

function Get-LocalDevelopmentVmOverrides {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return @{}
    }
    $settings = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    $overrides = @{}
    if (
        [string]::IsNullOrWhiteSpace($env:FLUTZ_LINUX_BRIDGE_ADAPTER) -and
        -not [string]::IsNullOrWhiteSpace([string]$settings.bridgeAdapter)
    ) {
        $overrides.BridgeAdapter = [string]$settings.bridgeAdapter
    }
    if (
        [string]::IsNullOrWhiteSpace($env:FLUTZ_LINUX_RDP_SOURCE_CIDR) -and
        -not [string]::IsNullOrWhiteSpace([string]$settings.rdpSourceCidr)
    ) {
        $overrides.RdpSourceCidr = [string]$settings.rdpSourceCidr
    }
    $overrides
}

function Get-VirtualBoxBridgedInterfaces {
    param([Parameter(Mandatory = $true)][string]$VBoxManage)

    $output = Invoke-DevelopmentVmTool `
        -Executable $VBoxManage `
        -Arguments @('list', 'bridgedifs') `
        -Description 'VirtualBox bridged-interface inventory' `
        -TimeoutSeconds 60
    $interfaces = @()
    foreach ($block in ($output -split '(?:\r?\n){2,}')) {
        $fields = @{}
        foreach ($line in ($block -split '\r?\n')) {
            if ($line -match '^([^:]+):\s*(.*)$') {
                $fields[$matches[1].Trim()] = $matches[2].Trim()
            }
        }
        if ($fields.Name) {
            $interfaces += [pscustomobject]@{
                Name = [string]$fields.Name
                Status = [string]$fields.Status
                IpAddress = [string]$fields.IPAddress
                HardwareAddress = [string]$fields.HardwareAddress
            }
        }
    }
    $interfaces
}

function Get-HostPrimaryIPv4Interface {
    $route = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -AddressFamily IPv4 -ErrorAction Stop |
        Where-Object { $_.NextHop -and $_.NextHop -ne '0.0.0.0' }
    if (-not $route) {
        return $null
    }
    $ranked = $route | ForEach-Object {
        $interfaceMetric = 0
        try {
            $interfaceMetric = [int](Get-NetIPInterface -InterfaceIndex $_.ifIndex -AddressFamily IPv4 -ErrorAction Stop).InterfaceMetric
        } catch {
            $interfaceMetric = 9999
        }
        [pscustomobject]@{
            InterfaceIndex = [int]$_.ifIndex
            TotalMetric = [int]$_.RouteMetric + $interfaceMetric
        }
    } | Sort-Object TotalMetric

    foreach ($candidate in $ranked) {
        $address = Get-NetIPAddress -InterfaceIndex $candidate.InterfaceIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue |
            Where-Object { $_.IPAddress -notlike '169.254.*' -and $_.IPAddress -ne '127.0.0.1' } |
            Select-Object -First 1
        $adapter = Get-NetAdapter -InterfaceIndex $candidate.InterfaceIndex -ErrorAction SilentlyContinue
        if ($address -and $adapter -and $adapter.Status -eq 'Up') {
            return [pscustomobject]@{
                InterfaceIndex = $candidate.InterfaceIndex
                IpAddress = [string]$address.IPAddress
                PrefixLength = [int]$address.PrefixLength
                InterfaceAlias = [string]$adapter.Name
                InterfaceDescription = [string]$adapter.InterfaceDescription
                MacAddress = ([string]$adapter.MacAddress).Replace('-', '').Replace(':', '').ToLowerInvariant()
            }
        }
    }
    $null
}

function Get-IPv4NetworkCidr {
    param(
        [Parameter(Mandatory = $true)][string]$IpAddress,
        [Parameter(Mandatory = $true)][int]$PrefixLength
    )

    if ($PrefixLength -lt 1 -or $PrefixLength -gt 32) {
        throw "Cannot derive a LAN CIDR from prefix length $PrefixLength."
    }
    $addressBytes = ([System.Net.IPAddress]::Parse($IpAddress)).GetAddressBytes()
    $networkBytes = New-Object byte[] 4
    for ($index = 0; $index -lt 4; $index++) {
        $bits = [Math]::Max(0, [Math]::Min(8, $PrefixLength - ($index * 8)))
        $maskByte = [byte](256 - [Math]::Pow(2, 8 - $bits))
        $networkBytes[$index] = [byte]($addressBytes[$index] -band $maskByte)
    }
    "$([System.Net.IPAddress]::new($networkBytes).ToString())/$PrefixLength"
}

function Resolve-DevelopmentVmNetworkDefaults {
    param(
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)][string]$VBoxManage
    )

    $defaults = @{}
    $bridgeMissing = [string]::IsNullOrWhiteSpace($Configuration.BridgeAdapter)
    $cidrMissing = [string]::IsNullOrWhiteSpace($Configuration.RdpSourceCidr)
    if (-not $bridgeMissing -and -not $cidrMissing) {
        return $defaults
    }

    $primary = Get-HostPrimaryIPv4Interface
    if (-not $primary) {
        throw 'No active IPv4 default-route interface was found on the host, so the bridged adapter and RDP source CIDR could not be detected automatically. Connect the host to its network, or set FLUTZ_LINUX_BRIDGE_ADAPTER and FLUTZ_LINUX_RDP_SOURCE_CIDR.'
    }

    if ($bridgeMissing) {
        $interfaces = @(Get-VirtualBoxBridgedInterfaces -VBoxManage $VBoxManage)
        $match = @($interfaces | Where-Object { $_.IpAddress -eq $primary.IpAddress })
        if ($match.Count -ne 1) {
            $match = @($interfaces | Where-Object {
                ([string]$_.HardwareAddress).Replace('-', '').Replace(':', '').ToLowerInvariant() -eq $primary.MacAddress
            })
        }
        if ($match.Count -ne 1) {
            $match = @($interfaces | Where-Object { $_.Name -eq $primary.InterfaceDescription })
        }
        if ($match.Count -ne 1) {
            $summary = if ($interfaces.Count) {
                ($interfaces | ForEach-Object { "'$($_.Name)' ($($_.IpAddress))" }) -join ', '
            } else {
                'none'
            }
            throw "The host's primary network interface '$($primary.InterfaceDescription)' ($($primary.IpAddress)) could not be matched to exactly one VirtualBox bridged adapter. Set FLUTZ_LINUX_BRIDGE_ADAPTER or bridgeAdapter in the ignored local VM settings. VirtualBox adapters: $summary"
        }
        $defaults.BridgeAdapter = $match[0].Name
        Write-Host "Auto-detected bridged adapter '$($match[0].Name)' from the host's primary internet connection ($($primary.IpAddress))."
    }

    if ($cidrMissing) {
        $defaults.RdpSourceCidr = Get-IPv4NetworkCidr -IpAddress $primary.IpAddress -PrefixLength $primary.PrefixLength
        Write-Host "Auto-detected RDP source CIDR '$($defaults.RdpSourceCidr)' from the host's primary internet connection."
    }

    $defaults
}

function Assert-DevelopmentVmNetworkConfiguration {
    param(
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)][string]$VBoxManage
    )

    $interfaces = @(Get-VirtualBoxBridgedInterfaces -VBoxManage $VBoxManage)
    $usable = @($interfaces | Where-Object {
        $_.Status -eq 'Up' -and
        -not [string]::IsNullOrWhiteSpace($_.IpAddress) -and
        $_.IpAddress -notin @('0.0.0.0', '169.254.0.0')
    })
    if ([string]::IsNullOrWhiteSpace($Configuration.BridgeAdapter)) {
        $summary = if ($usable.Count) {
            ($usable | ForEach-Object { "'$($_.Name)' ($($_.IpAddress))" }) -join ', '
        } else {
            'none'
        }
        throw "No bridged adapter was selected. Set FLUTZ_LINUX_BRIDGE_ADAPTER or bridgeAdapter in the ignored local VM settings. Usable VirtualBox adapters: $summary"
    }
    $selected = @($interfaces | Where-Object Name -eq $Configuration.BridgeAdapter)
    if ($selected.Count -ne 1) {
        throw "VirtualBox does not expose exactly one bridged adapter named '$($Configuration.BridgeAdapter)'."
    }
    if ($selected[0].Status -ne 'Up' -or $selected[0].IpAddress -in @('', '0.0.0.0')) {
        throw "The selected bridged adapter '$($Configuration.BridgeAdapter)' is not connected with a usable IPv4 address."
    }
    if ([string]::IsNullOrWhiteSpace($Configuration.RdpSourceCidr)) {
        throw 'RDP LAN exposure requires FLUTZ_LINUX_RDP_SOURCE_CIDR or rdpSourceCidr in the ignored local VM settings.'
    }
}

function New-DevelopmentVmCredentialFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Username = 'flutzplayer-tester'
    )

    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        $credential = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
        if (
            [string]::IsNullOrWhiteSpace([string]$credential.username) -or
            [string]::IsNullOrWhiteSpace([string]$credential.password)
        ) {
            throw "RDP credential file is invalid: $Path"
        }
        if ([string]$credential.username -eq $Username) {
            return $false
        }
        $credential.username = $Username
        $credential | ConvertTo-Json | Set-Content -LiteralPath $Path -Encoding utf8
        Write-Host "Migrated the dedicated RDP account name to '$Username'; the VM will be reprovisioned."
        return $true
    }
    if ($Username -notmatch '^[a-z_][a-z0-9_-]{2,31}$' -or $Username -in @('root', 'vagrant')) {
        throw "RDP test-account username '$Username' is invalid or reserved."
    }

    $characterGroups = @(
        'abcdefghijkmnopqrstuvwxyz',
        'ABCDEFGHJKLMNPQRSTUVWXYZ',
        '23456789',
        '!@#$%_-+='
    )
    $random = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $nextIndex = {
            param([int]$Maximum)
            $limit = 256 - (256 % $Maximum)
            $buffer = New-Object byte[] 1
            do {
                $random.GetBytes($buffer)
            } while ($buffer[0] -ge $limit)
            [int]($buffer[0] % $Maximum)
        }
        $passwordCharacters = [System.Collections.Generic.List[char]]::new()
        foreach ($group in $characterGroups) {
            1..2 | ForEach-Object {
                $passwordCharacters.Add($group[(& $nextIndex $group.Length)])
            }
        }
        $allCharacters = $characterGroups -join ''
        1..4 | ForEach-Object {
            $passwordCharacters.Add($allCharacters[(& $nextIndex $allCharacters.Length)])
        }
        for ($index = $passwordCharacters.Count - 1; $index -gt 0; $index--) {
            $swap = & $nextIndex ($index + 1)
            $temporary = $passwordCharacters[$index]
            $passwordCharacters[$index] = $passwordCharacters[$swap]
            $passwordCharacters[$swap] = $temporary
        }
        $password = -join $passwordCharacters
    } finally {
        $random.Dispose()
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    [ordered]@{
        username = $Username
        password = $password
        createdAtUtc = [DateTime]::UtcNow.ToString('o')
    } | ConvertTo-Json | Set-Content -LiteralPath $Path -Encoding utf8

    $currentIdentity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $acl = [System.Security.AccessControl.FileSecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($identity in @(
        $currentIdentity.User,
        [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18'),
        [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
    )) {
        $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
            $identity,
            [System.Security.AccessControl.FileSystemRights]::FullControl,
            [System.Security.AccessControl.AccessControlType]::Allow
        )
        [void]$acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
    $true
}

function Get-DevelopmentVmProvisioningFingerprint {
    $inputs = @(
        (Join-Path $script:LinuxDevelopmentVmRoot 'Vagrantfile'),
        (Join-Path $script:LinuxDevelopmentVmRoot 'tool-versions.json'),
        (Join-Path $script:LinuxDevelopmentVmRoot 'provision\provisioning-manifest.schema.json'),
        (Join-Path $script:LinuxDevelopmentVmRoot 'provision\provision.sh'),
        (Join-Path $script:LinuxDevelopmentVmRoot 'provision\configure-gnome-remote-desktop.sh'),
        (Join-Path $script:LinuxDevelopmentVmRoot 'provision\verify-environment.sh')
    )
    $identity = $inputs | ForEach-Object {
        if (-not (Test-Path -LiteralPath $_ -PathType Leaf)) {
            throw "Provisioning input is missing: $_"
        }
        "$(Split-Path -Leaf $_):$((Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant())"
    }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes(($identity -join "`n"))
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha256.ComputeHash($bytes)
    } finally {
        $sha256.Dispose()
    }
    -join ($hash | ForEach-Object { $_.ToString('x2') })
}

function Get-DevelopmentVmEnvironment {
    param(
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)][string]$CredentialPath,
        [Parameter(Mandatory = $true)][string]$Vagrant
    )

    $vagrantRoot = Split-Path -Parent (Split-Path -Parent $Vagrant)
    $vagrantSshBin = Join-Path $vagrantRoot 'embedded\usr\bin'
    if (-not (Test-Path -LiteralPath (Join-Path $vagrantSshBin 'ssh.exe') -PathType Leaf)) {
        throw "Vagrant's bundled SSH client was not found under '$vagrantSshBin'."
    }
    @{
        PATH = "$vagrantSshBin;$env:PATH"
        FLUTZ_LINUX_VM_NAME = $Configuration.Name
        FLUTZ_LINUX_VM_CPUS = [string]$Configuration.Cpus
        FLUTZ_LINUX_VM_MEMORY_MB = [string]$Configuration.MemoryMb
        FLUTZ_LINUX_VM_DISK_GB = [string]$Configuration.DiskGb
        FLUTZ_LINUX_GUEST_SOURCE_PATH = $Configuration.GuestSourcePath
        FLUTZ_LINUX_GUEST_ARTIFACT_PATH = $Configuration.GuestArtifactPath
        FLUTZ_LINUX_GUEST_MANIFEST_PATH = $Configuration.GuestManifestPath
        FLUTZ_LINUX_BRIDGE_ADAPTER = $Configuration.BridgeAdapter
        FLUTZ_LINUX_RDP_SOURCE_CIDR = $Configuration.RdpSourceCidr
        FLUTZ_LINUX_GUEST_HOSTNAME = $Configuration.Hostname
        FLUTZ_LINUX_RDP_CREDENTIAL_FILE = $CredentialPath
        FLUTZ_LINUX_PROVISIONING_FINGERPRINT = Get-DevelopmentVmProvisioningFingerprint
    }
}

function Test-DevelopmentVmEnvironment {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)][hashtable]$Environment,
        [switch]$ThrowOnFailure
    )

    $probe = "sudo /usr/local/sbin/flutzplayer-verify-environment --check-only --manifest '$($Configuration.GuestManifestPath)' --source '$($Configuration.GuestSourcePath)' --artifacts '$($Configuration.GuestArtifactPath)' --rdp-cidr '$($Configuration.RdpSourceCidr)' --fingerprint '$($Environment.FLUTZ_LINUX_PROVISIONING_FINGERPRINT)'"
    try {
        $output = Invoke-DevelopmentVmTool `
            -Executable $Vagrant `
            -Arguments @('ssh', '-c', $probe, '--', '-T') `
            -Description 'Development VM readiness checks' `
            -Environment $Environment `
            -TimeoutSeconds 900
        if ($output -notmatch 'ENVIRONMENT_OK hostname=(\S+) lan=(\d+\.\d+\.\d+\.\d+)') {
            return $null
        }
        [pscustomobject]@{
            Hostname = $matches[1]
            LanAddress = $matches[2]
        }
    } catch {
        if ($ThrowOnFailure) {
            throw
        }
        Write-Host "Development VM readiness check requires repair: $($_.Exception.Message)"
        $null
    }
}

function Get-DevelopmentVmMachineState {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][hashtable]$Environment
    )

    $machineIdPath = Join-Path $VagrantRoot '.vagrant\machines\default\virtualbox\id'
    if (-not (Test-Path -LiteralPath $machineIdPath -PathType Leaf)) {
        return [pscustomobject]@{
            State = 'absent'
            MachineId = $null
            MachineIdPath = $machineIdPath
        }
    }
    $machineId = (Get-Content -LiteralPath $machineIdPath -Raw).Trim()
    if ([string]::IsNullOrWhiteSpace($machineId)) {
        throw "Vagrant metadata is irrecoverably corrupt because '$machineIdPath' is empty. No VM was destroyed. Run build-init.ps1 --OS Linux --DestroyVm --Force after reviewing the reported removal scope."
    }
    try {
        $status = Invoke-DevelopmentVmTool `
            -Executable $Vagrant `
            -Arguments @('status', '--machine-readable') `
            -Description 'Development VM status' `
            -Environment $Environment `
            -WorkingDirectory $VagrantRoot `
            -TimeoutSeconds 120
    } catch {
        throw "Vagrant metadata for machine '$machineId' could not be inspected. No VM was destroyed. Repair Vagrant metadata or run build-init.ps1 --OS Linux --DestroyVm --Force after reviewing the removal scope. $($_.Exception.Message)"
    }
    $stateMatches = [regex]::Matches($status, '(?m)^[^,]*,default,state,([^,\r\n]+)')
    if ($stateMatches.Count -ne 1) {
        throw "Vagrant returned an ambiguous machine state for '$machineId'. No VM was destroyed. Repair Vagrant metadata or use explicit --DestroyVm --Force."
    }
    $state = $stateMatches[0].Groups[1].Value
    if ($state -notin @('running', 'poweroff', 'saved', 'aborted')) {
        throw "Vagrant machine '$machineId' has unsupported state '$state'. No VM was destroyed. Repair the state or use explicit --DestroyVm --Force."
    }
    [pscustomobject]@{
        State = $state
        MachineId = $machineId
        MachineIdPath = $machineIdPath
    }
}

$script:DevelopmentVmAudioSettings = [ordered]@{
    Driver = 'default'
    Controller = 'AC97'
    Output = 'on'
}

function Test-DevelopmentVmAudioConfiguration {
    param(
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$MachineId
    )

    $machineReadable = Invoke-DevelopmentVmTool `
        -Executable $VBoxManage `
        -Arguments @('showvminfo', $MachineId, '--machinereadable') `
        -Description 'Development VM audio configuration inventory' `
        -TimeoutSeconds 120
    $machineLines = @($machineReadable -split '\r?\n' | ForEach-Object { $_.Trim() })
    $details = Invoke-DevelopmentVmTool `
        -Executable $VBoxManage `
        -Arguments @('showvminfo', $MachineId) `
        -Description 'Development VM audio controller inventory' `
        -TimeoutSeconds 120

    $audioLine = [regex]::Match($details, '(?m)^Audio:\s+(.+)$')
    $controller = ''
    $enabled = $false
    if ($audioLine.Success) {
        $enabled = $audioLine.Groups[1].Value.Trim().StartsWith('enabled')
        $controllerMatch = [regex]::Match($audioLine.Groups[1].Value, 'Controller:\s*([A-Za-z0-9]+)')
        if ($controllerMatch.Success) {
            $controller = $controllerMatch.Groups[1].Value
        }
    }

    [pscustomobject]@{
        Enabled = $enabled
        Driver = ($machineLines | Where-Object { $_ -match '^audio="(.*)"$' } | ForEach-Object { $matches[1] } | Select-Object -First 1)
        Output = ($machineLines | Where-Object { $_ -match '^audio_out="(.*)"$' } | ForEach-Object { $matches[1] } | Select-Object -First 1)
        Controller = $controller
        IsCompliant = (
            $enabled -and
            $controller -eq $script:DevelopmentVmAudioSettings.Controller -and
            ($machineLines -contains "audio=`"$($script:DevelopmentVmAudioSettings.Driver)`"") -and
            ($machineLines -contains "audio_out=`"$($script:DevelopmentVmAudioSettings.Output)`"")
        )
    }
}

function Set-DevelopmentVmAudioConfiguration {
    param(
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][hashtable]$Environment,
        [Parameter(Mandatory = $true)]$Machine
    )

    $audio = Test-DevelopmentVmAudioConfiguration -VBoxManage $VBoxManage -MachineId $Machine.MachineId
    if ($audio.IsCompliant) {
        return $false
    }

    Write-Host "Development VM audio requires reconfiguration (enabled=$($audio.Enabled), driver='$($audio.Driver)', controller='$($audio.Controller)', output='$($audio.Output)'). VirtualBox applies audio changes only while the VM is powered off."
    if ($Machine.State -eq 'running') {
        Invoke-DevelopmentVmTool `
            -Executable $Vagrant `
            -Arguments @('halt') `
            -Description 'Development VM shutdown for audio reconfiguration' `
            -Environment $Environment `
            -WorkingDirectory $VagrantRoot `
            -TimeoutSeconds 600 | Out-Null
    }

    Invoke-DevelopmentVmTool `
        -Executable $VBoxManage `
        -Arguments @(
            'modifyvm', $Machine.MachineId,
            '--audio-driver', $script:DevelopmentVmAudioSettings.Driver,
            '--audio-controller', $script:DevelopmentVmAudioSettings.Controller.ToLowerInvariant(),
            '--audio-enabled', 'on',
            '--audio-out', $script:DevelopmentVmAudioSettings.Output
        ) `
        -Description 'Development VM audio reconfiguration' `
        -TimeoutSeconds 120 | Out-Null

    $applied = Test-DevelopmentVmAudioConfiguration -VBoxManage $VBoxManage -MachineId $Machine.MachineId
    if (-not $applied.IsCompliant) {
        throw "VirtualBox audio reconfiguration did not take effect (enabled=$($applied.Enabled), driver='$($applied.Driver)', controller='$($applied.Controller)', output='$($applied.Output)')."
    }
    Write-Host "Development VM audio is now enabled with the $($script:DevelopmentVmAudioSettings.Controller) controller, default host driver, and playback output enabled."
    $true
}

function Assert-DevelopmentVmVirtualBoxState {
    param(
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$MachineId,
        [Parameter(Mandatory = $true)]$Configuration
    )

    $info = Invoke-DevelopmentVmTool `
        -Executable $VBoxManage `
        -Arguments @('showvminfo', $MachineId, '--machinereadable') `
        -Description 'Development VM VirtualBox configuration checks' `
        -TimeoutSeconds 120
    $infoLines = @($info -split '\r?\n' | ForEach-Object { $_.Trim() })
    foreach ($expectation in @(
        "cpus=$($Configuration.Cpus)",
        "memory=$($Configuration.MemoryMb)"
    )) {
        if ($expectation -notin $infoLines) {
            throw "Development VM VirtualBox configuration is missing expected value: $expectation"
        }
    }
    $bridgedSlots = @(
        $infoLines | ForEach-Object {
            if ($_ -match '^nic(\d+)="bridged"$') {
                [int]$matches[1]
            }
        }
    )
    $matchingBridge = @($bridgedSlots | Where-Object {
        "bridgeadapter$_=`"$($Configuration.BridgeAdapter)`"" -in $infoLines
    })
    if ($matchingBridge.Count -eq 0) {
        throw "Development VM must have at least one bridged NIC attached to '$($Configuration.BridgeAdapter)'."
    }
    if ($info -match '(?m)^Forwarding\(\d+\)=.*,(?:3389),') {
        throw 'VirtualBox must not publish guest RDP through a NAT port-forward.'
    }
    $audio = Test-DevelopmentVmAudioConfiguration -VBoxManage $VBoxManage -MachineId $MachineId
    if (-not $audio.IsCompliant) {
        throw "Development VM audio must be enabled with the $($script:DevelopmentVmAudioSettings.Controller) controller, '$($script:DevelopmentVmAudioSettings.Driver)' host driver, and playback output enabled, but VirtualBox reports enabled=$($audio.Enabled), driver='$($audio.Driver)', controller='$($audio.Controller)', output='$($audio.Output)'."
    }
}

function Update-DevelopmentVmAddressState {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Health
    )

    $previous = $null
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        $previous = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    }
    if ($previous -and $previous.lanAddress -eq $Health.LanAddress -and $previous.hostname -eq $Health.Hostname) {
        return
    }
    if ($previous -and $previous.lanAddress -ne $Health.LanAddress) {
        Write-Warning "Linux development VM LAN address changed from $($previous.lanAddress) to $($Health.LanAddress)."
    }
    [ordered]@{
        hostname = $Health.Hostname
        lanAddress = $Health.LanAddress
        observedAtUtc = [DateTime]::UtcNow.ToString('o')
    } | ConvertTo-Json | Set-Content -LiteralPath $Path -Encoding utf8
}

function Initialize-LinuxDevelopmentVm {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][string]$WorkRoot,
        [Parameter(Mandatory = $true)]$Configuration,
        [Parameter(Mandatory = $true)]$MsysPaths
    )

    Assert-DevelopmentVmNetworkConfiguration -Configuration $Configuration -VBoxManage $VBoxManage
    New-Item -ItemType Directory -Force -Path $WorkRoot | Out-Null
    $credentialPath = Join-Path $WorkRoot 'credentials\rdp-test-account.json'
    $credentialCreated = New-DevelopmentVmCredentialFile -Path $credentialPath
    $environment = Get-DevelopmentVmEnvironment `
        -Configuration $Configuration `
        -CredentialPath $credentialPath `
        -Vagrant $Vagrant

    Push-Location $VagrantRoot
    try {
        $machine = Get-DevelopmentVmMachineState `
            -Vagrant $Vagrant `
            -VagrantRoot $VagrantRoot `
            -Environment $environment
        if ($machine.State -ne 'absent') {
            $audioChanged = Set-DevelopmentVmAudioConfiguration `
                -VBoxManage $VBoxManage `
                -Vagrant $Vagrant `
                -VagrantRoot $VagrantRoot `
                -Environment $environment `
                -Machine $machine
            if ($audioChanged) {
                $machine = Get-DevelopmentVmMachineState `
                    -Vagrant $Vagrant `
                    -VagrantRoot $VagrantRoot `
                    -Environment $environment
            }
            if ($machine.State -ne 'running') {
                Invoke-DevelopmentVmTool `
                    -Executable $Vagrant `
                    -Arguments @('up', '--no-provision', '--provider', 'virtualbox') `
                    -Description 'Development VM start' `
                    -Environment $environment `
                    -TimeoutSeconds 1800 | Out-Null
            }
            $sshConfiguration = Resolve-LinuxSshConfiguration `
                -Vagrant $Vagrant `
                -VagrantRoot $VagrantRoot `
                -Environment $environment `
                -OutputPath (Join-Path $WorkRoot 'ssh-config') `
                -MsysPaths $MsysPaths `
                -VBoxManage $VBoxManage `
                -MachineId $machine.MachineId `
                -TrustedCidr $Configuration.RdpSourceCidr
            $environment.FLUTZ_LINUX_SSH_HOST = $sshConfiguration.HostName
            $environment.FLUTZ_LINUX_SSH_PORT = [string]$sshConfiguration.Port
            $health = $null
            if ($credentialCreated) {
                Write-Host 'Local RDP credentials were regenerated, so the existing VM must be reprovisioned to apply them.'
            } elseif ($audioChanged) {
                Write-Host 'VirtualBox audio was reconfigured, so the VM must be revalidated before it is reported ready.'
            } else {
                $health = Test-DevelopmentVmEnvironment `
                    -Vagrant $Vagrant `
                    -Configuration $Configuration `
                    -Environment $environment
            }
            if ($health) {
                Assert-DevelopmentVmVirtualBoxState `
                    -VBoxManage $VBoxManage `
                    -MachineId $machine.MachineId `
                    -Configuration $Configuration
                Update-DevelopmentVmAddressState `
                    -Path (Join-Path $WorkRoot 'state.json') `
                    -Health $health
                return [pscustomobject]@{
                    State = 'ready'
                    Hostname = $health.Hostname
                    LanAddress = $health.LanAddress
                    CredentialPath = $credentialPath
                    Message = "Linux development VM is ready at $($health.Hostname) ($($health.LanAddress)); no provisioning changes were made. RDP credentials are stored in restricted local state."
                }
            }
        } else {
            Invoke-DevelopmentVmTool `
                -Executable $Vagrant `
                -Arguments @('up', '--no-provision', '--provider', 'virtualbox') `
                -Description 'Development VM creation and boot' `
                -Environment $environment `
                -TimeoutSeconds 1800 | Out-Null
            $machine = Get-DevelopmentVmMachineState `
                -Vagrant $Vagrant `
                -VagrantRoot $VagrantRoot `
                -Environment $environment
            $sshConfiguration = Resolve-LinuxSshConfiguration `
                -Vagrant $Vagrant `
                -VagrantRoot $VagrantRoot `
                -Environment $environment `
                -OutputPath (Join-Path $WorkRoot 'ssh-config') `
                -MsysPaths $MsysPaths `
                -VBoxManage $VBoxManage `
                -MachineId $machine.MachineId `
                -TrustedCidr $Configuration.RdpSourceCidr
            $environment.FLUTZ_LINUX_SSH_HOST = $sshConfiguration.HostName
            $environment.FLUTZ_LINUX_SSH_PORT = [string]$sshConfiguration.Port
        }

        $provisionLog = Join-Path $WorkRoot "logs\provision-$([DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')).log"
        Invoke-DevelopmentVmTool `
            -Executable $Vagrant `
            -Arguments @('provision') `
            -Description 'Development VM provisioning' `
            -Environment $environment `
            -LogPath $provisionLog `
            -TimeoutSeconds 28800 | Out-Null
        $health = Test-DevelopmentVmEnvironment `
            -Vagrant $Vagrant `
            -Configuration $Configuration `
            -Environment $environment `
            -ThrowOnFailure
        if (-not $health) {
            throw 'Development VM provisioning completed without a valid readiness result.'
        }
        $machine = Get-DevelopmentVmMachineState `
            -Vagrant $Vagrant `
            -VagrantRoot $VagrantRoot `
            -Environment $environment
        Assert-DevelopmentVmVirtualBoxState `
            -VBoxManage $VBoxManage `
            -MachineId $machine.MachineId `
            -Configuration $Configuration
        Update-DevelopmentVmAddressState `
            -Path (Join-Path $WorkRoot 'state.json') `
            -Health $health
        [pscustomobject]@{
            State = 'repaired'
            Hostname = $health.Hostname
            LanAddress = $health.LanAddress
            CredentialPath = $credentialPath
            Message = "Linux development VM was provisioned and is ready at $($health.Hostname) ($($health.LanAddress)). RDP credentials are stored in restricted local state."
        }
    } catch {
        throw "Linux development VM readiness or repair failed. The VM and its user state were preserved; no automatic rebuild or destruction was attempted. Diagnose the failure above, then repair it or explicitly run --DestroyVm --Force. $($_.Exception.Message)"
    } finally {
        Pop-Location
    }
}

function Clear-LinuxDevelopmentBuildState {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][string]$WorkRoot,
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)]$Configuration
    )

    $credentialPath = Join-Path $WorkRoot 'credentials\rdp-test-account.json'
    $environment = Get-DevelopmentVmEnvironment `
        -Configuration $Configuration `
        -CredentialPath $credentialPath `
        -Vagrant $Vagrant
    $machine = Get-DevelopmentVmMachineState `
        -Vagrant $Vagrant `
        -VagrantRoot $VagrantRoot `
        -Environment $environment
    $guestCleaned = $false
    if ($machine.State -ne 'absent') {
        if ($machine.State -ne 'running') {
            Invoke-DevelopmentVmTool `
                -Executable $Vagrant `
                -Arguments @('up', '--no-provision', '--provider', 'virtualbox') `
                -Description 'Development VM start for Linux clean' `
                -Environment $environment `
                -WorkingDirectory $VagrantRoot `
                -TimeoutSeconds 1800 | Out-Null
        }
        $guestPaths = @(
            $Configuration.GuestSourcePath,
            $Configuration.GuestArtifactPath,
            '/home/vagrant/flutzPlayer-staging'
        )
        $quotedPaths = $guestPaths | ForEach-Object {
            "'" + ([string]$_).Replace("'", "'`"`"'`"`'") + "'"
        }
        $cleanCommand = "for path in $($quotedPaths -join ' '); do if [ -d `"`$path`" ]; then sudo find `"`$path`" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +; fi; done"
        Invoke-DevelopmentVmTool `
            -Executable $Vagrant `
            -Arguments @('ssh', '-c', $cleanCommand, '--', '-T') `
            -Description 'Linux guest build-state clean' `
            -Environment $environment `
            -WorkingDirectory $VagrantRoot `
            -TimeoutSeconds 900 | Out-Null
        $guestCleaned = $true
    }

    $hostTargets = @(
        (Join-Path $RepoRoot 'drops\flutzPlayer-Linux'),
        (Join-Path $RepoRoot 'drops\flutzPlayer-AppImage'),
        (Join-Path $RepoRoot 'drops\flutzPlayer-Linux-Release'),
        (Join-Path $RepoRoot '_local\linux\build-output'),
        (Join-Path $RepoRoot '_local\linux\transfer')
    )
    $removedHostTargets = [System.Collections.Generic.List[string]]::new()
    foreach ($target in $hostTargets) {
        if (Test-Path -LiteralPath $target) {
            Remove-Item -LiteralPath $target -Recurse -Force
            $removedHostTargets.Add($target)
        }
    }
    $scope = if ($guestCleaned) { 'guest build/source/staging and host Linux output' } else { 'host Linux output; no VM exists' }
    [pscustomobject]@{
        State = 'repaired'
        RemovedHostTargets = @($removedHostTargets)
        Message = "Linux clean completed for $scope. The VM, local box, ISO, Cargo cache, pinned tools, settings, credentials, and lifecycle logs were preserved."
    }
}

function Remove-LinuxDevelopmentVm {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VagrantRoot,
        [Parameter(Mandatory = $true)][string]$WorkRoot,
        [Parameter(Mandatory = $true)]$Configuration
    )

    $machineIdPath = Join-Path $VagrantRoot '.vagrant\machines\default\virtualbox\id'
    $machineId = if (Test-Path -LiteralPath $machineIdPath -PathType Leaf) {
        (Get-Content -LiteralPath $machineIdPath -Raw).Trim()
    } else {
        $null
    }
    $credentialPath = Join-Path $WorkRoot 'credentials\rdp-test-account.json'
    $environment = Get-DevelopmentVmEnvironment `
        -Configuration $Configuration `
        -CredentialPath $credentialPath `
        -Vagrant $Vagrant
    Write-Host 'Explicit Linux VM destruction scope:'
    Write-Host "  Vagrant machine: $($Configuration.Name)"
    Write-Host "  VirtualBox machine ID: $(if ($machineId) { $machineId } else { '<none>' })"
    Write-Host "  Vagrant machine state: $(Join-Path $VagrantRoot '.vagrant\machines\default')"
    Write-Host "  Local VM credentials/state/logs: $WorkRoot"
    Write-Host '  Preserved: Ubuntu ISO cache, registered local Vagrant box, host prerequisites, and all non-VM caches.'

    if ($machineId) {
        Invoke-DevelopmentVmTool `
            -Executable $Vagrant `
            -Arguments @('destroy', '--force') `
            -Description 'Explicit Linux development VM destruction' `
            -Environment $environment `
            -WorkingDirectory $VagrantRoot `
            -TimeoutSeconds 1800 | Out-Null
    }
    foreach ($target in @(
        (Join-Path $VagrantRoot '.vagrant\machines\default'),
        (Join-Path $WorkRoot 'credentials'),
        (Join-Path $WorkRoot 'logs'),
        (Join-Path $WorkRoot 'state.json')
    )) {
        if (Test-Path -LiteralPath $target) {
            Remove-Item -LiteralPath $target -Recurse -Force
        }
    }
    [pscustomobject]@{
        State = if ($machineId) { 'repaired' } else { 'ready' }
        Message = if ($machineId) {
            'Linux development VM and its local machine state were destroyed explicitly. The Ubuntu ISO and registered local Vagrant box were preserved.'
        } else {
            'No Linux development VM exists. Stale local VM credentials/state/logs were removed; reusable ISO and box caches were preserved.'
        }
    }
}
