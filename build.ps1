param(
    [string]$OS = "Windows",

    [string]$Configuration = "Release",

    [switch]$SkipBuild,
    [switch]$RebuildDat,
    [switch]$BuildDat,
    [switch]$CleanDrop,
    [switch]$NoJemallocMemory,
    [ValidateRange(60, 86400)]
    [int]$LinuxCommandTimeoutSeconds = 14400,
    [ValidateRange(60, 86400)]
    [int]$LinuxTransferTimeoutSeconds = 1800,
    [ValidateRange(5, 300)]
    [int]$LinuxSshTimeoutSeconds = 30,

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$RemainingArguments
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$Remaining = @($RemainingArguments | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })

if ($OS -in @("--OS", "-OS")) {
    if ($Configuration -notin @("Debug", "Release")) {
        $OS = $Configuration
        $Configuration = "Release"
    } else {
        $OS = $Remaining[0]
        $Remaining = @($Remaining | Select-Object -Skip 1)
    }
} elseif ($OS.StartsWith("--OS=")) {
    $OS = $OS.Substring(5)
}

$normalizedRemaining = [System.Collections.Generic.List[string]]::new()
for ($index = 0; $index -lt $Remaining.Count; $index++) {
    $argument = $Remaining[$index]
    if ($argument -in @("--OS", "-OS")) {
        $index++
        if ($index -ge $Remaining.Count) {
            throw 'The --OS spelling requires exactly one value: --OS Windows, --OS Linux, or --OS Linux-AppImage'
        }
        $OS = $Remaining[$index]
    } elseif ($argument.StartsWith("--OS=")) {
        $OS = $argument.Substring(5)
    } else {
        $normalizedRemaining.Add($argument)
    }
}
$Remaining = @($normalizedRemaining)

if ($OS -notin @("Windows", "Linux", "Linux-AppImage")) {
    throw "Unsupported OS '$OS'. Expected Windows, Linux, or Linux-AppImage."
}
if ($Configuration -notin @("Debug", "Release")) {
    switch ($Configuration) {
        '-SkipBuild' { $SkipBuild = $true }
        '--SkipBuild' { $SkipBuild = $true }
        '-RebuildDat' { $RebuildDat = $true }
        '--RebuildDat' { $RebuildDat = $true }
        '-BuildDat' { $BuildDat = $true }
        '--BuildDat' { $BuildDat = $true }
        '-CleanDrop' { $CleanDrop = $true }
        '--CleanDrop' { $CleanDrop = $true }
        '-NoJemallocMemory' { $NoJemallocMemory = $true }
        '--NoJemallocMemory' { $NoJemallocMemory = $true }
        default { throw "Unsupported configuration '$Configuration'. Expected Debug or Release." }
    }
    $Configuration = "Release"
}

for ($index = 0; $index -lt $Remaining.Count; $index++) {
    if ([string]::IsNullOrWhiteSpace($Remaining[$index])) {
        continue
    }
    switch ($Remaining[$index]) {
        '-SkipBuild' { $SkipBuild = $true }
        '--SkipBuild' { $SkipBuild = $true }
        '-RebuildDat' { $RebuildDat = $true }
        '--RebuildDat' { $RebuildDat = $true }
        '-BuildDat' { $BuildDat = $true }
        '--BuildDat' { $BuildDat = $true }
        '-CleanDrop' { $CleanDrop = $true }
        '--CleanDrop' { $CleanDrop = $true }
        '-NoJemallocMemory' { $NoJemallocMemory = $true }
        '--NoJemallocMemory' { $NoJemallocMemory = $true }
        '-Configuration' {
            $index++
            if ($index -ge $Remaining.Count) {
                throw 'The -Configuration switch requires Debug or Release.'
            }
            $Configuration = $Remaining[$index]
        }
        '--Configuration' {
            $index++
            if ($index -ge $Remaining.Count) {
                throw 'The --Configuration switch requires Debug or Release.'
            }
            $Configuration = $Remaining[$index]
        }
        default { throw "Unexpected argument '$($Remaining[$index])'." }
    }
}

if ($OS -eq "Linux-AppImage" -and $Configuration -ne "Release") {
    throw 'Linux-AppImage packaging requires -Configuration Release. No build or drop changes were performed.'
}

if ($OS -in @('Linux', 'Linux-AppImage')) {
    . (Join-Path $RepoRoot 'build/linux/LinuxBuild.ps1')
    Invoke-LinuxBuild `
        -RepoRoot $RepoRoot `
        -OS $OS `
        -Configuration $Configuration `
        -SkipBuild:$SkipBuild `
        -RebuildDat:$RebuildDat `
        -BuildDat:$BuildDat `
        -CleanDrop:$CleanDrop `
        -NoJemallocMemory:$NoJemallocMemory `
        -CommandTimeoutSeconds $LinuxCommandTimeoutSeconds `
        -TransferTimeoutSeconds $LinuxTransferTimeoutSeconds `
        -SshTimeoutSeconds $LinuxSshTimeoutSeconds
    return
}

. (Join-Path $RepoRoot 'build/windows/WindowsBuild.ps1')
Invoke-WindowsBuild `
    -RepoRoot $RepoRoot `
    -Configuration $Configuration `
    -SkipBuild:$SkipBuild `
    -BuildDat:$BuildDat `
    -RebuildDat:$RebuildDat `
    -CleanDrop:$CleanDrop `
    -NoJemallocMemory:$NoJemallocMemory