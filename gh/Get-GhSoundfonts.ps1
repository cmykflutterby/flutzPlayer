param(
    [Parameter(Mandatory = $true)]
    [string]$DestinationDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

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

$SoundfontUrl = $env:FLUTZ_SOUNDFONT_URL
if ([string]::IsNullOrWhiteSpace($SoundfontUrl)) {
    throw 'FLUTZ_SOUNDFONT_URL environment variable is not set. Configure it as a GitHub Actions secret pointing to a zip archive containing the required soundfont files.'
}

New-Item -ItemType Directory -Force -Path $DestinationDirectory | Out-Null

$missingFonts = @($RequiredFonts | Where-Object { -not (Test-Path -LiteralPath (Join-Path $DestinationDirectory $_)) })
if ($missingFonts.Count -eq 0) {
    Write-Host "All required soundfonts already present in $DestinationDirectory."
    exit 0
}

Write-Host "Downloading soundfont archive from configured URL ($($missingFonts.Count) font(s) missing)"

$downloadRoot = Join-Path ([System.IO.Path]::GetTempPath()) 'flutz-gh-soundfonts'
if (Test-Path -LiteralPath $downloadRoot) {
    Remove-Item -Recurse -Force -LiteralPath $downloadRoot
}
New-Item -ItemType Directory -Force -Path $downloadRoot | Out-Null

$archivePath = Join-Path $downloadRoot 'soundfonts.zip'

Invoke-WebRequest `
    -Uri $SoundfontUrl `
    -OutFile $archivePath `
    -Headers @{ 'User-Agent' = 'Mozilla/5.0' } `
    -MaximumRedirection 10

if (-not (Test-Path -LiteralPath $archivePath)) {
    throw "Soundfont archive download did not produce a file at: $archivePath"
}

$archiveBytes = [System.IO.File]::ReadAllBytes($archivePath)
if ($archiveBytes.Length -lt 4 -or $archiveBytes[0] -ne 0x50 -or $archiveBytes[1] -ne 0x4B) {
    $preview = [System.Text.Encoding]::UTF8.GetString($archiveBytes, 0, [Math]::Min(512, $archiveBytes.Length))
    throw "Downloaded file is not a valid ZIP archive (bad magic bytes). The URL may require authentication or returned an error page.`nContent preview:`n$preview"
}

$extractRoot = Join-Path $downloadRoot 'extract'
New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null
Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot -Force

$restored = @()
foreach ($fontName in $missingFonts) {
    $match = Get-ChildItem -LiteralPath $extractRoot -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -ieq $fontName } |
        Select-Object -First 1

    if ($null -eq $match) {
        Write-Warning "Font not found in downloaded archive: $fontName"
        continue
    }

    Copy-Item -LiteralPath $match.FullName -Destination (Join-Path $DestinationDirectory $fontName) -Force
    $restored += $fontName
}

$stillMissing = @($RequiredFonts | Where-Object { -not (Test-Path -LiteralPath (Join-Path $DestinationDirectory $_)) })
if ($stillMissing.Count -gt 0) {
    $formatted = ($stillMissing | Sort-Object) -join [System.Environment]::NewLine
    throw "Soundfont acquisition incomplete after download. Still missing:`n$formatted"
}

Write-Host "Acquired $($restored.Count) soundfont(s)."
