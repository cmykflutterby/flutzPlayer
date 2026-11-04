$script:LinuxBuildConfigSchemaId = 'https://flutzplayer.local/schemas/linux-build-config/v1'
$script:LinuxProvisioningSchemaId = 'https://flutzplayer.local/schemas/linux-provisioning-manifest/v1'

function Get-LinuxBuildConfiguration {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [hashtable]$Overrides = @{}
    )

    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        throw "Linux build configuration not found: $ConfigPath"
    }

    $config = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($config.schemaId -ne $script:LinuxBuildConfigSchemaId) {
        throw "Unsupported Linux build configuration schema '$($config.schemaId)'. Expected '$script:LinuxBuildConfigSchemaId'."
    }
    if ($config.provisioningSchemaId -ne $script:LinuxProvisioningSchemaId) {
        throw "Unsupported provisioning schema '$($config.provisioningSchemaId)'. Expected '$script:LinuxProvisioningSchemaId'."
    }
    if ($config.vm.ubuntuRelease -ne '26.04.1' -or $config.vm.architecture -ne 'x86_64') {
        throw 'Linux builds require Ubuntu 26.04.1 on x86_64.'
    }

    $resolved = [ordered]@{
        Name = [string]$config.vm.name
        Cpus = [int]$config.vm.cpus
        MemoryMb = [int]$config.vm.memoryMb
        DiskGb = [int]$config.vm.diskGb
        GuestSourcePath = [string]$config.vm.guestSourcePath
        GuestArtifactPath = [string]$config.vm.guestArtifactPath
        GuestManifestPath = [string]$config.vm.guestManifestPath
        BridgeAdapter = [string]$config.vm.network.bridgeAdapter
        LanAddressMode = [string]$config.vm.network.lanAddressMode
        RdpSourceCidr = [string]$config.vm.network.rdpSourceCidr
        Hostname = [string]$config.vm.network.hostname
    }

    foreach ($property in @($resolved.Keys)) {
        $environmentName = [string]$config.overrideEnvironmentVariables.$property
        if ($environmentName) {
            $environmentValue = [Environment]::GetEnvironmentVariable($environmentName)
            if (-not [string]::IsNullOrWhiteSpace($environmentValue)) {
                $resolved[$property] = $environmentValue
            }
        }
        if ($Overrides.ContainsKey($property) -and $null -ne $Overrides[$property] -and $Overrides[$property] -ne '') {
            $resolved[$property] = $Overrides[$property]
        }
    }

    foreach ($property in @('Cpus', 'MemoryMb', 'DiskGb')) {
        $parsed = 0
        if (-not [int]::TryParse([string]$resolved[$property], [ref]$parsed)) {
            throw "Linux VM $property must be an integer."
        }
        $ruleName = $property.Substring(0, 1).ToLowerInvariant() + $property.Substring(1)
        $rule = $config.validation.$ruleName
        if ($parsed -lt [int]$rule.minimum -or $parsed -gt [int]$rule.maximum) {
            throw "Linux VM $property must be between $($rule.minimum) and $($rule.maximum)."
        }
        $resolved[$property] = $parsed
    }

    if ($resolved.LanAddressMode -notin @($config.validation.lanAddressModes)) {
        throw "Linux VM LanAddressMode must be one of: $($config.validation.lanAddressModes -join ', ')."
    }
    if ($resolved.LanAddressMode -eq 'static') {
        throw 'Static LAN addressing requires an explicit static address contract, which is not defined in Phase 1. Use dhcp.'
    }
    foreach ($property in @('Name', 'GuestSourcePath', 'GuestArtifactPath', 'GuestManifestPath', 'Hostname')) {
        if ([string]::IsNullOrWhiteSpace([string]$resolved[$property])) {
            throw "Linux VM $property cannot be empty."
        }
    }
    if ($resolved.Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,62}$') {
        throw 'Linux VM Name must be 1-63 characters using letters, digits, dots, underscores, or hyphens.'
    }
    if ($resolved.Hostname -notmatch '^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$') {
        throw 'Linux VM Hostname must be a valid lowercase DNS label.'
    }
    if (
        -not $resolved.GuestSourcePath.StartsWith('/') -or
        -not $resolved.GuestArtifactPath.StartsWith('/') -or
        -not $resolved.GuestManifestPath.StartsWith('/')
    ) {
        throw 'Linux guest source, artifact, and manifest paths must be absolute POSIX paths.'
    }
    if (
        $resolved.GuestSourcePath -match '(^|/)\.\.(/|$)' -or
        $resolved.GuestArtifactPath -match '(^|/)\.\.(/|$)' -or
        $resolved.GuestManifestPath -match '(^|/)\.\.(/|$)'
    ) {
        throw 'Linux guest source, artifact, and manifest paths cannot contain parent-directory segments.'
    }
    if (-not [string]::IsNullOrWhiteSpace($resolved.RdpSourceCidr)) {
        $cidrParts = $resolved.RdpSourceCidr.Split('/')
        $address = $null
        $prefixLength = 0
        if (
            $cidrParts.Count -ne 2 -or
            -not [System.Net.IPAddress]::TryParse($cidrParts[0], [ref]$address) -or
            $address.AddressFamily -ne [System.Net.Sockets.AddressFamily]::InterNetwork -or
            -not [int]::TryParse($cidrParts[1], [ref]$prefixLength) -or
            $prefixLength -lt 0 -or
            $prefixLength -gt 32
        ) {
            throw 'Linux VM RdpSourceCidr must be an IPv4 CIDR, for example 192.168.1.0/24.'
        }
    }

    [pscustomobject]$resolved
}

function Get-LinuxBuildExitStates {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath
    )

    $config = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    $states = [ordered]@{
        Ready = [int]$config.exitStates.ready
        Fatal = [int]$config.exitStates.fatal
        Repaired = [int]$config.exitStates.repaired
        RestartRequired = [int]$config.exitStates.restartRequired
    }
    if ($states.Ready -ne 0) {
        throw 'The ready exit state must be 0.'
    }
    if (@($states.Values | Select-Object -Unique).Count -ne $states.Count) {
        throw 'Linux build exit-state codes must be unique.'
    }
    foreach ($state in $states.GetEnumerator()) {
        if ($state.Value -lt 0 -or $state.Value -gt 255) {
            throw "Linux build exit state '$($state.Key)' must be between 0 and 255."
        }
    }
    $states
}
