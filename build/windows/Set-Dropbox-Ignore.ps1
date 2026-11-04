<#

Set-Dropbox-Ignore

Useful to set windows DB client to ignore massive _local/build cache locations

# Toggle the current state
.\Set-Dropbox-Ignore.ps1 -Path 'C:\Users\you\Dropbox\Reports'

# Force the item to be ignored
.\Set-Dropbox-Ignore.ps1 -Path 'C:\Users\you\Dropbox\Reports' -Ignore

# Force the item to stop being ignored
.\Set-Dropbox-Ignore.ps1 -Path 'C:\Users\you\Dropbox\Reports' -StopIgnoring

# List explicitly marked items; auto-detect Dropbox roots
.\Set-Dropbox-Ignore.ps1 -List

# List explicitly marked items under a specified root
.\Set-Dropbox-Ignore.ps1 -List -Root 'C:\Users\you\Dropbox'

#>
[CmdletBinding(DefaultParameterSetName = 'Path')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Path')]
    [string]$Path,

    [Parameter(ParameterSetName = 'Path')]
    [switch]$Ignore,

    [Parameter(ParameterSetName = 'Path')]
    [switch]$StopIgnoring,

    [Parameter(Mandatory = $true, ParameterSetName = 'List')]
    [switch]$List,

    [Parameter(ParameterSetName = 'List')]
    [string[]]$Root
)

$streamName = 'com.dropbox.ignored'

function Test-DropboxIgnored {
    param([Parameter(Mandatory = $true)][string]$LiteralPath)

    $stream = Get-Item -LiteralPath $LiteralPath `
        -Stream $script:streamName -ErrorAction SilentlyContinue

    return ($null -ne $stream)
}

if ($PSCmdlet.ParameterSetName -eq 'Path') {
    if ($Ignore -and $StopIgnoring) {
        throw 'Use either -Ignore or -StopIgnoring, not both.'
    }

    # Resolve the target; the ADS can only be changed on an existing item.
    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    $currentlyIgnored = Test-DropboxIgnored -LiteralPath $item.FullName

    if ($Ignore) {
        $shouldIgnore = $true
    }
    elseif ($StopIgnoring) {
        $shouldIgnore = $false
    }
    else {
        # No state flag: toggle the current state.
        $shouldIgnore = -not $currentlyIgnored
    }

    if ($shouldIgnore -eq $currentlyIgnored) {
        if ($shouldIgnore) {
            Write-Output "Already ignored: $($item.FullName)"
        }
        else {
            Write-Output "Already not ignored: $($item.FullName)"
        }
        return
    }

    if ($shouldIgnore) {
        Set-Content -LiteralPath $item.FullName `
            -Stream $streamName -Value '1' -NoNewline -ErrorAction Stop
        Write-Output "Now ignored: $($item.FullName)"
    }
    else {
        Clear-Content -LiteralPath $item.FullName `
            -Stream $streamName -ErrorAction Stop
        Write-Output "No longer ignored: $($item.FullName)"
    }

    return
}

# List mode: use the supplied roots, or find Dropbox account roots in its config.
if (-not $Root -or $Root.Count -eq 0) {
    $Root = @()
    $infoPath = Join-Path $env:APPDATA 'Dropbox\info.json'

    if (Test-Path -LiteralPath $infoPath -PathType Leaf) {
        try {
            $info = Get-Content -LiteralPath $infoPath -Raw |
                ConvertFrom-Json -ErrorAction Stop

            foreach ($account in $info.PSObject.Properties) {
                $accountPath = $account.Value.path
                if ($accountPath -and
                    (Test-Path -LiteralPath $accountPath -PathType Container)) {
                    $Root += $accountPath
                }
            }
        }
        catch {
            Write-Warning "Could not read Dropbox account locations from $infoPath."
        }
    }
}

$Root = @($Root | Select-Object -Unique)

if (-not $Root -or $Root.Count -eq 0) {
    throw 'No Dropbox root was found. Use -List -Root "C:\path\to\Dropbox".'
}

$found = $false

foreach ($scanRoot in $Root) {
    if (-not (Test-Path -LiteralPath $scanRoot -PathType Container)) {
        Write-Warning "Root not found or is not a folder: $scanRoot"
        continue
    }

    $rootItem = Get-Item -LiteralPath $scanRoot -ErrorAction SilentlyContinue
    if ($rootItem -and (Test-DropboxIgnored -LiteralPath $rootItem.FullName)) {
        Write-Output $rootItem.FullName
        $found = $true
    }

    Get-ChildItem -LiteralPath $scanRoot -Force -Recurse -ErrorAction SilentlyContinue |
        ForEach-Object {
            if (Test-DropboxIgnored -LiteralPath $_.FullName) {
                Write-Output $_.FullName
                $script:found = $true
            }
        }
}

if (-not $found) {
    Write-Output 'No explicitly marked ignored items found.'
}
