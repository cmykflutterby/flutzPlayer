$script:UbuntuBoxName = 'flutzplayer/ubuntu-26.04.1-x86_64'
$script:UbuntuIsoName = 'ubuntu-26.04.1-live-server-amd64.iso'
$script:UbuntuIsoUrl = "https://releases.ubuntu.com/26.04.1/$script:UbuntuIsoName"
$script:UbuntuIsoSha256 = 'cc8a95cde20f6ced61a322420de00f10cc3c90ced545daa46cb9c1a117f1d927'
$script:UbuntuChecksumsUrl = 'https://releases.ubuntu.com/26.04.1/SHA256SUMS'
$script:UbuntuBoxStampSchemaId = 'https://flutzplayer.local/schemas/ubuntu-base-box-stamp/v1'

function Invoke-BoxTool {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Description,
        [hashtable]$Environment = @{}
    )

    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    Write-Host "[$([DateTime]::UtcNow.ToString('o'))] Starting: $Description"
    $previous = @{}
    foreach ($entry in $Environment.GetEnumerator()) {
        $previous[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key, 'Process')
        [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, 'Process')
    }
    try {
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $output = & $Executable @Arguments 2>&1 | Out-String
            $exitCode = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousPreference
        }
    } finally {
        foreach ($entry in $previous.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
        }
    }
    if ($exitCode -ne 0) {
        Write-Host "[$([DateTime]::UtcNow.ToString('o'))] Failed after $([Math]::Round($stopwatch.Elapsed.TotalSeconds, 1))s: $Description"
        throw "$Description failed with exit code $exitCode.`n$output"
    }
    Write-Host "[$([DateTime]::UtcNow.ToString('o'))] Completed in $([Math]::Round($stopwatch.Elapsed.TotalSeconds, 1))s: $Description"
    $output.Trim()
}

function Get-UbuntuBoxPaths {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$WorkRoot
    )

    [pscustomobject]@{
        PackerRoot = Join-Path $RepoRoot 'build\linux\packer'
        Template = Join-Path $RepoRoot 'build\linux\packer\ubuntu-26.04.1.pkr.hcl'
        IsoRoot = Join-Path $WorkRoot 'iso'
        Iso = Join-Path $WorkRoot "iso\$script:UbuntuIsoName"
        Checksums = Join-Path $WorkRoot 'iso\SHA256SUMS'
        PackerCache = Join-Path $WorkRoot 'packer-cache'
        PackerConfig = Join-Path $WorkRoot 'packer-config'
        PackerOutput = Join-Path $WorkRoot 'packer-output\ubuntu-26.04.1-x86_64'
        BoxRoot = Join-Path $WorkRoot 'boxes'
        Box = Join-Path $WorkRoot 'boxes\ubuntu-26.04.1-x86_64-virtualbox.box'
        BuildFingerprint = Join-Path $WorkRoot 'boxes\ubuntu-26.04.1-x86_64-build-inputs.sha256'
        ValidationRoot = Join-Path $WorkRoot 'validation'
        ValidationHome = Join-Path $WorkRoot 'validation\vagrant-home'
        ValidationVm = Join-Path $WorkRoot 'validation\vm'
        ValidationStamp = Join-Path $WorkRoot 'validation\ubuntu-26.04.1-x86_64.json'
        Logs = Join-Path $WorkRoot 'logs'
    }
}

function Test-RegisteredUbuntuBox {
    param([Parameter(Mandatory = $true)][string]$Vagrant)

    $output = Invoke-BoxTool -Executable $Vagrant -Arguments @('box', 'list') -Description 'Vagrant box inventory'
    [regex]::IsMatch($output, "(?m)^$([regex]::Escape($script:UbuntuBoxName))\s+\(virtualbox,")
}

function Assert-OfficialUbuntuChecksum {
    param(
        [Parameter(Mandatory = $true)][string]$Curl,
        [Parameter(Mandatory = $true)]$Paths
    )

    New-Item -ItemType Directory -Force -Path $Paths.IsoRoot | Out-Null
    Invoke-BoxTool -Executable $Curl -Arguments @(
        '--fail', '--location', '--retry', '5', '--silent', '--show-error',
        '--output', $Paths.Checksums, $script:UbuntuChecksumsUrl
    ) -Description 'Ubuntu official checksum download' | Out-Null

    $expectedLine = "$script:UbuntuIsoSha256 *$script:UbuntuIsoName"
    $checksumLines = @(Get-Content -LiteralPath $Paths.Checksums)
    if ($expectedLine -notin $checksumLines) {
        throw "Ubuntu's official SHA256SUMS does not contain the pinned checksum for $script:UbuntuIsoName."
    }
}

function Get-VerifiedUbuntuIso {
    param(
        [Parameter(Mandatory = $true)][string]$Curl,
        [Parameter(Mandatory = $true)]$Paths
    )

    Assert-OfficialUbuntuChecksum -Curl $Curl -Paths $Paths
    if (Test-Path -LiteralPath $Paths.Iso -PathType Leaf) {
        $existingHash = (Get-FileHash -LiteralPath $Paths.Iso -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($existingHash -eq $script:UbuntuIsoSha256) {
            return $false
        }
        Remove-Item -LiteralPath $Paths.Iso -Force
    }

    Invoke-BoxTool -Executable $Curl -Arguments @(
        '--fail', '--location', '--retry', '5', '--continue-at', '-',
        '--output', $Paths.Iso, $script:UbuntuIsoUrl
    ) -Description 'Ubuntu 26.04.1 ISO download' | Out-Null
    $downloadedHash = (Get-FileHash -LiteralPath $Paths.Iso -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($downloadedHash -ne $script:UbuntuIsoSha256) {
        throw "Ubuntu ISO checksum mismatch. Expected $script:UbuntuIsoSha256, got $downloadedHash."
    }
    $true
}

function Initialize-PackerTemplate {
    param(
        [Parameter(Mandatory = $true)][string]$Packer,
        [Parameter(Mandatory = $true)]$Paths
    )

    New-Item -ItemType Directory -Force -Path $Paths.PackerConfig, $Paths.PackerCache, $Paths.Logs | Out-Null
    $environment = @{
        PACKER_CONFIG_DIR = $Paths.PackerConfig
        PACKER_CACHE_DIR = $Paths.PackerCache
    }
    Invoke-BoxTool -Executable $Packer -Arguments @('init', $Paths.PackerRoot) -Description 'Packer plugin initialization' -Environment $environment | Out-Null
    Invoke-BoxTool -Executable $Packer -Arguments @(
        'validate',
        "-var=iso_path=$($Paths.Iso)",
        "-var=output_directory=$($Paths.PackerOutput)",
        "-var=box_output_path=$($Paths.Box)",
        $Paths.Template
    ) -Description 'Packer Ubuntu template validation' -Environment $environment | Out-Null
}

function Get-BoxBuildInputFingerprint {
    param([Parameter(Mandatory = $true)]$Paths)

    $inputs = @(
        $Paths.Template,
        (Join-Path $Paths.PackerRoot 'http\meta-data'),
        (Join-Path $Paths.PackerRoot 'http\user-data'),
        (Join-Path $Paths.PackerRoot 'scripts\prepare-vagrant-box.sh')
    )
    $hashLines = $inputs | ForEach-Object {
        if (-not (Test-Path -LiteralPath $_ -PathType Leaf)) {
            throw "Packer build input is missing: $_"
        }
        "$_|$((Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant())"
    }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($hashLines -join "`n")
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        ([System.BitConverter]::ToString($sha256.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
}

function Test-BoxBuildInputFingerprint {
    param([Parameter(Mandatory = $true)]$Paths)

    if (-not (Test-Path -LiteralPath $Paths.BuildFingerprint -PathType Leaf)) {
        return $false
    }
    (Get-Content -LiteralPath $Paths.BuildFingerprint -Raw).Trim() -eq (Get-BoxBuildInputFingerprint -Paths $Paths)
}

function Build-UbuntuBox {
    param(
        [Parameter(Mandatory = $true)][string]$Packer,
        [Parameter(Mandatory = $true)]$Paths
    )

    New-Item -ItemType Directory -Force -Path $Paths.BoxRoot, $Paths.Logs | Out-Null
    if (Test-Path -LiteralPath $Paths.PackerOutput) {
        Remove-Item -LiteralPath $Paths.PackerOutput -Recurse -Force
    }
    if (Test-Path -LiteralPath $Paths.Box) {
        Remove-Item -LiteralPath $Paths.Box -Force
    }
    $environment = @{
        PACKER_CONFIG_DIR = $Paths.PackerConfig
        PACKER_CACHE_DIR = $Paths.PackerCache
        PACKER_LOG = '1'
        PACKER_LOG_PATH = Join-Path $Paths.Logs 'packer-build.log'
    }
    Invoke-BoxTool -Executable $Packer -Arguments @(
        'build', '-on-error=cleanup',
        "-var=iso_path=$($Paths.Iso)",
        "-var=output_directory=$($Paths.PackerOutput)",
        "-var=box_output_path=$($Paths.Box)",
        $Paths.Template
    ) -Description 'Packer Ubuntu 26.04.1 box build' -Environment $environment | Out-Null
    if (-not (Test-Path -LiteralPath $Paths.Box -PathType Leaf)) {
        throw "Packer did not produce the expected box: $($Paths.Box)"
    }
    $fingerprint = Get-BoxBuildInputFingerprint -Paths $Paths
    $temporaryFingerprint = "$($Paths.BuildFingerprint).tmp"
    [System.IO.File]::WriteAllText($temporaryFingerprint, "$fingerprint`n", [System.Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporaryFingerprint -Destination $Paths.BuildFingerprint -Force
}

function Write-VerificationVagrantfile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$BoxName
    )

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    $content = @"
Vagrant.configure("2") do |config|
  config.vm.box = "$BoxName"
  config.vm.boot_timeout = 900
  config.vm.synced_folder ".", "/vagrant", disabled: true
  config.ssh.insert_key = false
  config.vm.provider "virtualbox" do |vb|
    vb.name = "flutzplayer-ubuntu-26-04-1-box-verification"
    vb.cpus = 2
    vb.memory = 4096
  end
end
"@
    [System.IO.File]::WriteAllText($Path, $content, [System.Text.UTF8Encoding]::new($false))
}

function Test-UbuntuBoxArtifact {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)]$Paths
    )

    if (-not (Test-Path -LiteralPath $Paths.Box -PathType Leaf)) {
        return $false
    }

    New-Item -ItemType Directory -Force -Path $Paths.ValidationHome, $Paths.ValidationVm | Out-Null
    $validationName = 'flutzplayer-validation/ubuntu-26.04.1-x86_64'
    $vagrantRoot = Split-Path -Parent (Split-Path -Parent $Vagrant)
    $vagrantSshBin = Join-Path $vagrantRoot 'embedded\usr\bin'
    $vagrantSsh = Join-Path $vagrantSshBin 'ssh.exe'
    if (-not (Test-Path -LiteralPath $vagrantSsh -PathType Leaf)) {
        throw "Vagrant's bundled SSH client was not found at '$vagrantSsh'."
    }
    $environment = @{
        VAGRANT_HOME = $Paths.ValidationHome
        PATH = "$vagrantSshBin;$env:PATH"
    }
    Invoke-BoxTool -Executable $Vagrant -Arguments @(
        'box', 'add', '--force', '--name', $validationName, $Paths.Box
    ) -Description 'Isolated Vagrant box import' -Environment $environment | Out-Null
    Write-VerificationVagrantfile -Path (Join-Path $Paths.ValidationVm 'Vagrantfile') -BoxName $validationName

    Push-Location $Paths.ValidationVm
    try {
        Invoke-BoxTool -Executable $Vagrant -Arguments @('up', '--provider', 'virtualbox') -Description 'Verification VM boot' -Environment $environment | Out-Null
        $probe = @'
set -eu
. /etc/os-release
test "$ID" = ubuntu
test "$VERSION_ID" = "26.04"
grep -F "26.04.1" /var/log/installer/media-info
test "$(uname -m)" = x86_64
disk_bytes="$(lsblk -b -d -n -o SIZE | sort -nr | head -n1)"
test "$disk_bytes" -ge 85899345920
systemctl is-active --quiet ssh
printf 'BOX_VALIDATION_OK release=%s arch=%s disk=%s\n' "$VERSION_ID" "$(uname -m)" "$disk_bytes"
'@
        $output = Invoke-BoxTool -Executable $Vagrant -Arguments @('ssh', '--no-tty', '-c', $probe) -Description 'Verification VM health checks' -Environment $environment
        if ($output -notmatch 'BOX_VALIDATION_OK') {
            throw "Verification VM did not report success.`n$output"
        }
    } finally {
        try {
            Invoke-BoxTool -Executable $Vagrant -Arguments @('destroy', '--force') -Description 'Verification VM cleanup' -Environment $environment | Out-Null
        } finally {
            Pop-Location
        }
    }

    $stamp = [ordered]@{
        schemaId = 'https://flutzplayer.local/schemas/ubuntu-base-box-validation/v1'
        boxName = $script:UbuntuBoxName
        provider = 'virtualbox'
        ubuntuRelease = '26.04.1'
        architecture = 'x86_64'
        minimumDiskBytes = 85899345920
        boxSha256 = (Get-FileHash -LiteralPath $Paths.Box -Algorithm SHA256).Hash.ToLowerInvariant()
        validatedAtUtc = [DateTime]::UtcNow.ToString('o')
    }
    $temporaryStamp = "$($Paths.ValidationStamp).tmp"
    $stamp | ConvertTo-Json | Set-Content -LiteralPath $temporaryStamp -Encoding utf8
    Move-Item -LiteralPath $temporaryStamp -Destination $Paths.ValidationStamp -Force
    $true
}

function Test-UbuntuBoxValidationStamp {
    param([Parameter(Mandatory = $true)]$Paths)

    if (
        -not (Test-Path -LiteralPath $Paths.Box -PathType Leaf) -or
        -not (Test-Path -LiteralPath $Paths.ValidationStamp -PathType Leaf)
    ) {
        return $false
    }
    try {
        $stamp = Get-Content -LiteralPath $Paths.ValidationStamp -Raw | ConvertFrom-Json
        $boxHash = (Get-FileHash -LiteralPath $Paths.Box -Algorithm SHA256).Hash.ToLowerInvariant()
        $stamp.schemaId -eq 'https://flutzplayer.local/schemas/ubuntu-base-box-validation/v1' -and
            $stamp.boxName -eq $script:UbuntuBoxName -and
            $stamp.provider -eq 'virtualbox' -and
            $stamp.ubuntuRelease -eq '26.04.1' -and
            $stamp.architecture -eq 'x86_64' -and
            $stamp.boxSha256 -eq $boxHash
    } catch {
        $false
    }
}

function Get-VagrantBoxProviderDirectory {
    $vagrantHome = $env:VAGRANT_HOME
    if ([string]::IsNullOrWhiteSpace($vagrantHome)) {
        $vagrantHome = Join-Path $env:USERPROFILE '.vagrant.d'
    }
    $escapedName = $script:UbuntuBoxName.Replace('/', '-VAGRANTSLASH-')
    $boxRoot = Join-Path $vagrantHome "boxes\$escapedName"
    if (-not (Test-Path -LiteralPath $boxRoot -PathType Container)) {
        return $null
    }
    $providerDirectories = @(
        Get-ChildItem -LiteralPath $boxRoot -Directory -Recurse -Depth 2 -Filter 'virtualbox' -ErrorAction SilentlyContinue |
            ForEach-Object { $_.FullName }
    )
    if ($providerDirectories.Count -eq 0) {
        return $null
    }
    $providerDirectories |
        Sort-Object { (Get-Item -LiteralPath $_).LastWriteTimeUtc } -Descending |
        Select-Object -First 1
}

function Write-RegisteredBoxStamp {
    param([Parameter(Mandatory = $true)][string]$BuildInputsSha256)

    $providerDirectory = Get-VagrantBoxProviderDirectory
    if (-not $providerDirectory) {
        return $false
    }
    [ordered]@{
        schemaId = $script:UbuntuBoxStampSchemaId
        boxName = $script:UbuntuBoxName
        provider = 'virtualbox'
        ubuntuRelease = '26.04.1'
        architecture = 'x86_64'
        buildInputsSha256 = $BuildInputsSha256
        stampedAtUtc = [DateTime]::UtcNow.ToString('o')
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $providerDirectory 'flutzplayer-build-inputs.json') -Encoding utf8
    $true
}

function Test-RegisteredBoxStamp {
    param([Parameter(Mandatory = $true)][string]$BuildInputsSha256)

    $providerDirectory = Get-VagrantBoxProviderDirectory
    if (-not $providerDirectory) {
        return $false
    }
    $stampPath = Join-Path $providerDirectory 'flutzplayer-build-inputs.json'
    if (-not (Test-Path -LiteralPath $stampPath -PathType Leaf)) {
        return $false
    }
    try {
        $stamp = Get-Content -LiteralPath $stampPath -Raw | ConvertFrom-Json
        $stamp.schemaId -eq $script:UbuntuBoxStampSchemaId -and
            $stamp.boxName -eq $script:UbuntuBoxName -and
            $stamp.provider -eq 'virtualbox' -and
            $stamp.ubuntuRelease -eq '26.04.1' -and
            $stamp.architecture -eq 'x86_64' -and
            $stamp.buildInputsSha256 -eq $BuildInputsSha256
    } catch {
        $false
    }
}

function Register-UbuntuBox {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)]$Paths
    )

    $arguments = @('box', 'add', '--name', $script:UbuntuBoxName, '--provider', 'virtualbox')
    if (Test-RegisteredUbuntuBox -Vagrant $Vagrant) {
        $arguments += '--force'
    }
    $arguments += $Paths.Box
    Invoke-BoxTool -Executable $Vagrant -Arguments $arguments -Description 'Validated Vagrant box registration' | Out-Null
    if (-not (Test-RegisteredUbuntuBox -Vagrant $Vagrant)) {
        throw "Vagrant did not register '$script:UbuntuBoxName'."
    }
    $true
}

function Initialize-UbuntuBaseBox {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$WorkRoot,
        [Parameter(Mandatory = $true)][string]$Packer,
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$Curl
    )

    $paths = Get-UbuntuBoxPaths -RepoRoot $RepoRoot -WorkRoot $WorkRoot
    $buildInputs = Get-BoxBuildInputFingerprint -Paths $paths
    $registered = Test-RegisteredUbuntuBox -Vagrant $Vagrant
    if ($registered -and (Test-RegisteredBoxStamp -BuildInputsSha256 $buildInputs)) {
        return [pscustomobject]@{
            State = 'ready'
            BoxName = $script:UbuntuBoxName
            Message = "Validated Vagrant box '$script:UbuntuBoxName' is already registered and matches the current build inputs; the Ubuntu ISO and Packer were not required."
        }
    }

    $buildInputsCurrent = Test-BoxBuildInputFingerprint -Paths $paths
    if (
        $buildInputsCurrent -and
        $registered -and
        (Test-UbuntuBoxValidationStamp -Paths $paths)
    ) {
        Write-RegisteredBoxStamp -BuildInputsSha256 $buildInputs | Out-Null
        return [pscustomobject]@{
            State = 'ready'
            BoxName = $script:UbuntuBoxName
            Message = "Validated Vagrant box '$script:UbuntuBoxName' is already registered; Packer was not run."
        }
    }

    $changed = Get-VerifiedUbuntuIso -Curl $Curl -Paths $paths
    Initialize-PackerTemplate -Packer $Packer -Paths $paths

    if (-not $buildInputsCurrent -or -not (Test-Path -LiteralPath $paths.Box -PathType Leaf)) {
        Build-UbuntuBox -Packer $Packer -Paths $paths
        $changed = $true
    }
    if (-not (Test-UbuntuBoxValidationStamp -Paths $paths)) {
        Test-UbuntuBoxArtifact -Vagrant $Vagrant -Paths $paths | Out-Null
        $changed = $true
    }
    if (Register-UbuntuBox -Vagrant $Vagrant -Paths $paths) {
        $changed = $true
    }
    Write-RegisteredBoxStamp -BuildInputsSha256 $buildInputs | Out-Null

    [pscustomobject]@{
        State = if ($changed) { 'repaired' } else { 'ready' }
        BoxName = $script:UbuntuBoxName
        Message = "Validated Vagrant box '$script:UbuntuBoxName' is registered and ready."
    }
}
