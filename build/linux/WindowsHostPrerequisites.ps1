$script:HostPrerequisitesSchemaId = 'https://flutzplayer.local/schemas/windows-host-prerequisites/v1'

function Read-HostPrerequisiteConfig {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Host prerequisite configuration not found: $Path"
    }
    $config = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($config.schemaId -ne $script:HostPrerequisitesSchemaId) {
        throw "Unsupported host prerequisite schema '$($config.schemaId)'. Expected '$script:HostPrerequisitesSchemaId'."
    }
    $config
}

function Update-ProcessPath {
    $segments = [System.Collections.Generic.List[string]]::new()
    foreach ($scope in @('Machine', 'User', 'Process')) {
        $value = [Environment]::GetEnvironmentVariable('Path', $scope)
        foreach ($segment in @($value -split ';')) {
            if (-not [string]::IsNullOrWhiteSpace($segment) -and $segment -notin $segments) {
                $segments.Add($segment)
            }
        }
    }
    $env:Path = $segments -join ';'
}

function Resolve-HostExecutable {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [string[]]$CandidatePaths = @()
    )

    $resolved = Get-Command $Command -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($resolved) {
        return $resolved.Source
    }
    foreach ($candidate in $CandidatePaths) {
        $expanded = [Environment]::ExpandEnvironmentVariables($candidate)
        if (Test-Path -LiteralPath $expanded -PathType Leaf) {
            return $expanded
        }
    }
    $null
}

function Invoke-HostTool {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Description,
        [int[]]$SuccessExitCodes = @(0)
    )

    # Tools such as ssh -V report on stderr; with $ErrorActionPreference = 'Stop' PowerShell would
    # convert that stderr into a terminating NativeCommandError before the exit code is inspected.
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $Executable @Arguments 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($exitCode -notin $SuccessExitCodes) {
        throw "$Description failed with exit code $exitCode.`n$output"
    }
    $output.Trim()
}

function Test-SupportedPowerShell {
    param([Parameter(Mandatory = $true)]$Config)

    $minimum = if ($PSVersionTable.PSEdition -eq 'Desktop') {
        [version]$Config.powershell.desktopMinimum
    } elseif ($PSVersionTable.PSEdition -eq 'Core') {
        [version]$Config.powershell.coreMinimum
    } else {
        throw "Unsupported PowerShell edition '$($PSVersionTable.PSEdition)'."
    }
    if ($PSVersionTable.PSVersion -lt $minimum) {
        throw "PowerShell $($PSVersionTable.PSEdition) $minimum or newer is required; found $($PSVersionTable.PSVersion)."
    }
}

function Test-HardwareVirtualization {
    if ($env:OS -ne 'Windows_NT') {
        throw 'Linux VM host provisioning must run on Windows.'
    }

    $computer = Get-CimInstance Win32_ComputerSystem
    $processors = @(Get-CimInstance Win32_Processor)
    $available = [bool]$computer.HypervisorPresent -or
        ($processors.Count -gt 0 -and @($processors | Where-Object {
            $_.VirtualizationFirmwareEnabled -or $_.VMMonitorModeExtensions
        }).Count -eq $processors.Count)
    if (-not $available) {
        throw 'Hardware virtualization is unavailable. Enable Intel VT-x/AMD-V in firmware, reboot Windows, and rerun build-init.ps1 --OS Linux.'
    }
}

function Get-WingetExecutable {
    $winget = Resolve-HostExecutable -Command 'winget'
    if (-not $winget) {
        throw 'winget is required. Install or repair Microsoft App Installer, then rerun build-init.ps1 --OS Linux.'
    }
    Invoke-HostTool -Executable $winget -Arguments @('--version') -Description 'winget version probe' | Out-Null
    $winget
}

function Assert-WingetPackageIdentity {
    param(
        [Parameter(Mandatory = $true)][string]$Winget,
        [Parameter(Mandatory = $true)]$Package
    )

    $output = Invoke-HostTool -Executable $Winget -Arguments @(
        'show', '--id', $Package.id, '--exact', '--source', 'winget',
        '--accept-source-agreements', '--disable-interactivity'
    ) -Description "winget identity lookup for $($Package.id)"

    if ($output -notmatch "(?m)^Found .+ \[$([regex]::Escape($Package.id))\]\s*$") {
        throw "winget returned an unexpected package for '$($Package.id)'."
    }
    $publisherMatch = [regex]::Match($output, '(?m)^Publisher:\s*(.+?)\s*$')
    if (-not $publisherMatch.Success -or $publisherMatch.Groups[1].Value -ne $Package.publisher) {
        throw "Publisher verification failed for '$($Package.id)'. Expected '$($Package.publisher)'."
    }
}

function Install-WingetPackage {
    param(
        [Parameter(Mandatory = $true)][string]$Winget,
        [Parameter(Mandatory = $true)]$Package
    )

    Assert-WingetPackageIdentity -Winget $Winget -Package $Package
    Invoke-HostTool -Executable $Winget -Arguments @(
        'install', '--id', $Package.id, '--exact', '--source', 'winget',
        '--silent', '--accept-package-agreements', '--accept-source-agreements',
        '--disable-interactivity'
    ) -Description "winget installation of $($Package.name)" | Out-Null
    Update-ProcessPath
}

function Test-WingetPackageInstalled {
    param(
        [Parameter(Mandatory = $true)][string]$Winget,
        [Parameter(Mandatory = $true)]$Package
    )

    $output = & $Winget @(
        'list', '--id', $Package.id, '--exact', '--source', 'winget',
        '--accept-source-agreements', '--disable-interactivity'
    ) 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        return $false
    }
    $output -match "(?m)^\s*.+\s+$([regex]::Escape($Package.id))\s+.+$"
}

function Get-MsysPaths {
    param([Parameter(Mandatory = $true)]$Config)

    $root = [string]$Config.msys2.root
    [pscustomobject]@{
        Root = $root
        Bash = Join-Path $root 'usr\bin\bash.exe'
        Pacman = Join-Path $root 'usr\bin\pacman.exe'
        Cygpath = Join-Path $root 'usr\bin\cygpath.exe'
        Rsync = Join-Path $root 'usr\bin\rsync.exe'
        Ssh = Join-Path $root 'usr\bin\ssh.exe'
        Scp = Join-Path $root 'usr\bin\scp.exe'
    }
}

function Invoke-MsysCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Bash,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$Description
    )

    $previousArgConversion = $env:MSYS2_ARG_CONV_EXCL
    $previousMsystem = $env:MSYSTEM
    $previousChere = $env:CHERE_INVOKING
    try {
        $env:MSYS2_ARG_CONV_EXCL = '*'
        $env:MSYSTEM = 'MINGW64'
        $env:CHERE_INVOKING = '1'
        Invoke-HostTool -Executable $Bash -Arguments @('-lc', $Command) -Description $Description
    } finally {
        $env:MSYS2_ARG_CONV_EXCL = $previousArgConversion
        $env:MSYSTEM = $previousMsystem
        $env:CHERE_INVOKING = $previousChere
    }
}

function Invoke-PacmanQuery {
    param(
        [Parameter(Mandatory = $true)][string]$Pacman,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    # pacman writes diagnostics to stderr for absent packages; with $ErrorActionPreference = 'Stop'
    # PowerShell would convert that stderr into a terminating NativeCommandError.
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $Pacman @Arguments 2>&1
        [pscustomobject]@{
            ExitCode = $LASTEXITCODE
            Lines = @($output | Where-Object { $_ -isnot [System.Management.Automation.ErrorRecord] } | ForEach-Object { [string]$_ })
        }
    } finally {
        $ErrorActionPreference = $previousPreference
    }
}

function Get-MissingMsysPackages {
    param(
        [Parameter(Mandatory = $true)]$Config,
        [Parameter(Mandatory = $true)]$Paths
    )

    $missing = [System.Collections.Generic.List[string]]::new()
    foreach ($package in $Config.msys2.packages) {
        if ((Invoke-PacmanQuery -Pacman $Paths.Pacman -Arguments @('-Q', $package)).ExitCode -eq 0) {
            continue
        }

        $group = Invoke-PacmanQuery -Pacman $Paths.Pacman -Arguments @('-Sg', $package)
        if ($group.ExitCode -eq 0 -and $group.Lines.Count -gt 0) {
            $groupComplete = $true
            foreach ($groupLine in $group.Lines) {
                $member = ($groupLine -split '\s+')[-1]
                if ([string]::IsNullOrWhiteSpace($member)) {
                    continue
                }
                if ((Invoke-PacmanQuery -Pacman $Paths.Pacman -Arguments @('-Q', $member)).ExitCode -ne 0) {
                    $groupComplete = $false
                    break
                }
            }
            if ($groupComplete) {
                continue
            }
        }
        if ($package -notin $missing) {
            $missing.Add([string]$package)
        }
    }
    @($missing)
}

function Install-MsysPackages {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)][string[]]$Packages
    )

    Invoke-MsysCommand -Bash $Paths.Bash -Command 'pacman -Sy --noconfirm' -Description 'MSYS2 package metadata refresh' | Out-Null
    Invoke-MsysCommand -Bash $Paths.Bash -Command 'pacman -Syu --noconfirm' -Description 'MSYS2 core/system update stage one' | Out-Null
    Invoke-MsysCommand -Bash $Paths.Bash -Command 'pacman -Su --noconfirm' -Description 'MSYS2 core/system update stage two' | Out-Null
    $packageList = $Packages -join ' '
    Invoke-MsysCommand -Bash $Paths.Bash -Command "pacman -S --needed --noconfirm $packageList" -Description 'MSYS2 prerequisite package installation' | Out-Null
}

function Test-MsysTools {
    param(
        [Parameter(Mandatory = $true)]$Config,
        [Parameter(Mandatory = $true)]$Paths
    )

    foreach ($path in @($paths.Bash, $paths.Pacman, $paths.Cygpath, $paths.Rsync, $paths.Ssh, $paths.Scp)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Required MSYS2 executable is missing: $path"
        }
    }
    Invoke-HostTool -Executable $Paths.Bash -Arguments @('--version') -Description 'MSYS2 bash probe' | Out-Null
    Invoke-HostTool -Executable $Paths.Rsync -Arguments @('--version') -Description 'MSYS2 rsync probe' | Out-Null
    Invoke-HostTool -Executable $Paths.Ssh -Arguments @('-V') -Description 'MSYS2 OpenSSH probe' | Out-Null
    $commands = $Config.msys2.requiredCommands -join ' '
    Invoke-MsysCommand -Bash $Paths.Bash -Command "for command in $commands; do command -v `"`$command`" >/dev/null || exit 31; done" -Description 'MSYS2 required command probe' | Out-Null
}

function Test-VagrantVirtualBoxCompatibility {
    param(
        [Parameter(Mandatory = $true)][string]$Vagrant,
        [Parameter(Mandatory = $true)][string]$VBoxManage,
        [Parameter(Mandatory = $true)][string]$WorkRoot
    )

    $vagrantVersion = Invoke-HostTool -Executable $Vagrant -Arguments @('--version') -Description 'Vagrant version probe'
    $virtualBoxVersion = Invoke-HostTool -Executable $VBoxManage -Arguments @('--version') -Description 'VirtualBox version probe'
    Invoke-HostTool -Executable $VBoxManage -Arguments @('list', 'bridgedifs') -Description 'VirtualBox bridged-network probe' | Out-Null

    $probeRoot = Join-Path $WorkRoot 'vagrant-provider-probe'
    New-Item -ItemType Directory -Force -Path $probeRoot | Out-Null
    $vagrantfile = Join-Path $probeRoot 'Vagrantfile'
    [System.IO.File]::WriteAllText($vagrantfile, @'
Vagrant.configure("2") do |config|
  config.vm.box = "flutzplayer-phase2-probe-not-installed"
  config.vm.provider "virtualbox"
end
'@, [System.Text.UTF8Encoding]::new($false))
    Push-Location $probeRoot
    try {
        $output = & $Vagrant status --machine-readable 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    if ($exitCode -ne 0 -or $output -match 'VirtualBoxInvalidVersion|not supported by this version of Vagrant') {
        throw "Vagrant '$vagrantVersion' is not compatible with VirtualBox '$virtualBoxVersion'.`n$output"
    }
}

function Test-MsysPathAndRsync {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$WorkRoot
    )

    $convertedRepo = Invoke-HostTool -Executable $Paths.Cygpath -Arguments @('-u', $RepoRoot) -Description 'MSYS2 repository path conversion'
    if ($RepoRoot.Contains(' ') -and -not $convertedRepo.Contains(' ')) {
        throw "MSYS2 path conversion did not preserve the repository path space: $convertedRepo"
    }

    $probeRoot = Join-Path $WorkRoot 'rsync path probe'
    $source = Join-Path $probeRoot 'source'
    $destination = Join-Path $probeRoot 'destination'
    New-Item -ItemType Directory -Force -Path $source, $destination | Out-Null
    Set-Content -LiteralPath (Join-Path $source 'phase2 marker.txt') -Value 'phase2'
    $sourceMsys = Invoke-HostTool -Executable $Paths.Cygpath -Arguments @('-u', $source) -Description 'MSYS2 rsync source conversion'
    $destinationMsys = Invoke-HostTool -Executable $Paths.Cygpath -Arguments @('-u', $destination) -Description 'MSYS2 rsync destination conversion'
    $command = "rsync --dry-run --itemize-changes -a -- '$sourceMsys/' '$destinationMsys/'"
    $output = Invoke-MsysCommand -Bash $Paths.Bash -Command $command -Description 'MSYS2 rsync path-with-spaces dry run'
    if ($output -notmatch 'phase2 marker\.txt') {
        throw "The rsync dry run did not preserve the path-with-spaces marker.`n$output"
    }
}

function Initialize-WindowsHostPrerequisites {
    param(
        [Parameter(Mandatory = $true)][string]$ConfigPath,
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$WorkRoot
    )

    $config = Read-HostPrerequisiteConfig -Path $ConfigPath
    Test-SupportedPowerShell -Config $config
    Test-HardwareVirtualization
    $winget = Get-WingetExecutable
    $changed = $false

    foreach ($package in $config.wingetPackages) {
        $executable = Resolve-HostExecutable -Command $package.command -CandidatePaths $package.candidatePaths
        if (-not $executable) {
            Assert-WingetPackageIdentity -Winget $winget -Package $package
            if (Test-WingetPackageInstalled -Winget $winget -Package $package) {
                return [pscustomobject]@{
                    State = 'restartRequired'
                    Changed = $false
                    Message = "$($package.name) is installed, but '$($package.command)' is not discoverable. Restart Windows or sign out, then rerun build-init.ps1 --OS Linux."
                }
            }
            Write-Host "Installing missing host prerequisite: $($package.name)"
            Install-WingetPackage -Winget $winget -Package $package
            $changed = $true
            $executable = Resolve-HostExecutable -Command $package.command -CandidatePaths $package.candidatePaths
            if (-not $executable) {
                return [pscustomobject]@{
                    State = 'restartRequired'
                    Changed = $true
                    Message = "$($package.name) was installed, but '$($package.command)' is not discoverable. Restart Windows or sign out, then rerun build-init.ps1 --OS Linux."
                }
            }
        }
    }

    $paths = Get-MsysPaths -Config $config
    $missingPackages = @(Get-MissingMsysPackages -Config $config -Paths $paths)
    if ($missingPackages.Count -gt 0) {
        Write-Host "Installing missing MSYS2 packages: $($missingPackages -join ', ')"
        Install-MsysPackages -Paths $paths -Packages $missingPackages
        $changed = $true
    }
    Test-MsysTools -Config $config -Paths $paths

    $virtualBoxPackage = $config.wingetPackages | Where-Object name -eq 'VirtualBox'
    $vagrantPackage = $config.wingetPackages | Where-Object name -eq 'Vagrant'
    $packerPackage = $config.wingetPackages | Where-Object name -eq 'Packer'
    $vboxManage = Resolve-HostExecutable -Command $virtualBoxPackage.command -CandidatePaths $virtualBoxPackage.candidatePaths
    $vagrant = Resolve-HostExecutable -Command $vagrantPackage.command -CandidatePaths $vagrantPackage.candidatePaths
    $packer = Resolve-HostExecutable -Command $packerPackage.command -CandidatePaths $packerPackage.candidatePaths
    Invoke-HostTool -Executable $packer -Arguments @('--version') -Description 'Packer version probe' | Out-Null
    Test-VagrantVirtualBoxCompatibility -Vagrant $vagrant -VBoxManage $vboxManage -WorkRoot $WorkRoot
    Test-MsysPathAndRsync -Paths $paths -RepoRoot $RepoRoot -WorkRoot $WorkRoot

    [pscustomobject]@{
        State = if ($changed) { 'repaired' } else { 'ready' }
        Changed = $changed
        Message = if ($changed) {
            'Windows host prerequisites were installed or repaired and all health checks passed.'
        } else {
            'Windows host prerequisites are installed and all health checks passed; no package changes were needed.'
        }
    }
}
