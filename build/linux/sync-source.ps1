param(
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$LinuxRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent (Split-Path -Parent $LinuxRoot)
$ConfigPath = Join-Path $LinuxRoot 'config.json'
$HostConfigPath = Join-Path $LinuxRoot 'host-prerequisites.json'
$VmWorkRoot = Join-Path $RepoRoot '_local\linux\development-vm'
$SyncWorkRoot = Join-Path $RepoRoot '_local\linux\source-sync'

. (Join-Path $LinuxRoot 'LinuxBuildContract.ps1')
. (Join-Path $LinuxRoot 'LinuxDevelopmentVm.ps1')
. (Join-Path $LinuxRoot 'WindowsHostPrerequisites.ps1')
. (Join-Path $LinuxRoot 'LinuxSourceSync.ps1')

$localOverrides = Get-LocalDevelopmentVmOverrides -Path (Join-Path $VmWorkRoot 'settings.json')
$configuration = Get-LinuxBuildConfiguration -ConfigPath $ConfigPath -Overrides $localOverrides
$hostConfig = Read-HostPrerequisiteConfig -Path $HostConfigPath
$vagrantPackage = $hostConfig.wingetPackages | Where-Object name -eq 'Vagrant'
$virtualBoxPackage = $hostConfig.wingetPackages | Where-Object name -eq 'VirtualBox'
$vagrant = Resolve-HostExecutable -Command $vagrantPackage.command -CandidatePaths $vagrantPackage.candidatePaths
$vboxManage = Resolve-HostExecutable -Command $virtualBoxPackage.command -CandidatePaths $virtualBoxPackage.candidatePaths
if (-not $vagrant -or -not $vboxManage) {
    throw 'Vagrant and VirtualBox are required. Run build-init.ps1 --OS Linux first.'
}

$networkDefaults = Resolve-DevelopmentVmNetworkDefaults -Configuration $configuration -VBoxManage $vboxManage
if ($networkDefaults.Count -gt 0) {
    foreach ($entry in $networkDefaults.GetEnumerator()) {
        $localOverrides[$entry.Key] = $entry.Value
    }
    $configuration = Get-LinuxBuildConfiguration -ConfigPath $ConfigPath -Overrides $localOverrides
}

$credentialPath = Join-Path $VmWorkRoot 'credentials\rdp-test-account.json'
if (-not (Test-Path -LiteralPath $credentialPath -PathType Leaf)) {
    throw "Linux VM credentials are missing. Run build-init.ps1 --OS Linux first: $credentialPath"
}
$environment = Get-DevelopmentVmEnvironment `
    -Configuration $configuration `
    -CredentialPath $credentialPath `
    -Vagrant $vagrant
$machine = Get-DevelopmentVmMachineState `
    -Vagrant $vagrant `
    -VagrantRoot $LinuxRoot `
    -Environment $environment
if ($machine.State -ne 'running') {
    throw "The Linux development VM is '$($machine.State)', not running. Run build-init.ps1 --OS Linux first."
}

$msysPaths = Get-MsysPaths -Config $hostConfig
$result = Sync-LinuxSourceToVm `
    -RepoRoot $RepoRoot `
    -Vagrant $vagrant `
    -VagrantRoot $LinuxRoot `
    -Environment $environment `
    -Configuration $configuration `
    -MsysPaths $msysPaths `
    -VBoxManage $vboxManage `
    -MachineId $machine.MachineId `
    -WorkRoot $SyncWorkRoot `
    -DryRun:$DryRun

Write-Host "Linux source synchronization state: $($result.State); changed items: $($result.ChangedItemCount); log: $($result.LogPath)"
if (-not $DryRun) {
    Write-Host "Guest snapshot: $($result.SnapshotPath) ($($result.SnapshotSha256))"
}
