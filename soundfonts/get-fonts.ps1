[CmdletBinding()]
param(
    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$DownloadRoot = Join-Path ([System.IO.Path]::GetTempPath()) 'flutz-soundfonts-download'
$DropboxFolderUrl = 'https://www.dropbox.com/scl/fo/836g10pf6esxjlmc0c8ds/AIdCVveJE03AMpr47auM4fI?rlkey=8bz19qtfijxh2z0bjsj9tswur&st=bno1017i&dl=1'
$RequiredFonts = @(
    '8bitsf.SF2',
    'Arachno_SoundFont_Version_1.0.sf2',
    'FluidR3_GM2-2.SF2',
    'Four_Peters_Soundfont.sf2',
    'OPL-3 FM 128M.sf2',
    'Roland_SC-55_v2.2_by_Patch93_and_xan1242.sf2',
    'SONiVOX_GS250.sf2',
    'Sound.Blaster.Restoration.Project.sf2',
    'Super_Nintendo_Unofficial_update.sf2',
    'The_Ultimate Megadrive_Soundfont[v1.5].sf2',
    'The_Ultimate_Wii_Soundfont_V1-1.sf2',
    'Timbres Of Heaven GM_GS_XG_SFX V 3.4 Final.sf2',
    'TimGM6mb.sf2',
    'Yamaha_XG_Sound_Set.sf2'
)

function Write-Step {
    param([Parameter(Mandatory = $true)][string]$Message)

    Write-Host $Message
}

function New-Directory {
    param([Parameter(Mandatory = $true)][string]$Path)

    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Remove-PathIfPresent {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (Test-Path -LiteralPath $Path) {
        Remove-Item -Recurse -Force -LiteralPath $Path
    }
}

function Get-MissingFonts {
    $missingFonts = @()
    foreach ($fontName in $RequiredFonts) {
        $candidatePath = Join-Path $ScriptRoot $fontName
        if (-not (Test-Path -LiteralPath $candidatePath)) {
            $missingFonts += $fontName
        }
    }

    return $missingFonts
}

function Get-SourceArchivePath {
    New-Directory -Path $DownloadRoot
    return (Join-Path $DownloadRoot 'soundfonts.zip')
}

function Invoke-SoundfontDownload {
    param([Parameter(Mandatory = $true)][string]$DownloadUrl)

    $archivePath = Get-SourceArchivePath
    Remove-PathIfPresent -Path $archivePath

    Write-Step "Downloading soundfont archive from Dropbox"
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $archivePath -Headers @{ 'User-Agent' = 'Mozilla/5.0' }

    if (-not (Test-Path -LiteralPath $archivePath)) {
        throw "Soundfont archive download did not produce a file: $archivePath"
    }

    return $archivePath
}

function Expand-DownloadedArchive {
    param([Parameter(Mandatory = $true)][string]$ArchivePath)

    $extractRoot = Join-Path $DownloadRoot 'extract'
    Remove-PathIfPresent -Path $extractRoot
    New-Directory -Path $extractRoot

    Expand-Archive -LiteralPath $ArchivePath -DestinationPath $extractRoot -Force
    return $extractRoot
}

function Find-FontInArchive {
    param(
        [Parameter(Mandatory = $true)][string]$ExtractRoot,
        [Parameter(Mandatory = $true)][string]$FontName
    )

    $fontMatch = Get-ChildItem -LiteralPath $ExtractRoot -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -ieq $FontName } |
        Select-Object -First 1

    return $fontMatch
}

$missingFonts = Get-MissingFonts
if ($missingFonts.Count -eq 0) {
    Write-Step 'All required soundfonts are already present.'
    return
}

if ($ValidateOnly) {
    $formattedMissing = ($missingFonts | Sort-Object) -join [Environment]::NewLine
    throw "Missing soundfont file(s):`n$formattedMissing"
}

$archivePath = Invoke-SoundfontDownload -DownloadUrl $DropboxFolderUrl
$extractRoot = Expand-DownloadedArchive -ArchivePath $archivePath

$restoredFonts = @()
foreach ($fontName in $missingFonts) {
    $fontSource = Find-FontInArchive -ExtractRoot $extractRoot -FontName $fontName
    if ($null -eq $fontSource) {
        continue
    }

    Copy-Item -LiteralPath $fontSource.FullName -Destination (Join-Path $ScriptRoot $fontName) -Force
    $restoredFonts += $fontName
}

$remainingMissingFonts = Get-MissingFonts
if ($remainingMissingFonts.Count -gt 0) {
    $formattedMissing = ($remainingMissingFonts | Sort-Object) -join [Environment]::NewLine
    throw "Soundfont bootstrap incomplete. Missing file(s):`n$formattedMissing"
}

Write-Step "Restored $($restoredFonts.Count) missing soundfont(s)."