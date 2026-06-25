param(
  [string]$OutputDir = $PSScriptRoot,
  [switch]$IncludeHighResWav,
  [switch]$ListOnly,
  [switch]$ValidateUrls
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

<#
Source Sites

| Source | What it gives you | Best use |
|---|---|---|
| Free Audios Sample Files | Mono, stereo, 5.1/7.1, multiple sample rates including 44.1/48/96 kHz, and formats like MP3, WAV, FLAC, AAC, OGG, AIFF. Direct-download testing-focused files. <citation src="1"></citation> | Broad codec/container coverage |
| Espressif audio samples | Mono and stereo files, plus sample-rate variants; formats include AAC, AC3, AIFF, FLAC, M4A, MP3, OGG, Opus, WAV, WMA. Includes 44.1 kHz examples and sample-rate-specific MP3 sets. <citation src="2"></citation> | Simple reproducible format/channel matrix |
| MSB bit-perfect testing | Standard WAV test files at 44.1, 48, 96 kHz and beyond, with multiple bit depths. <citation src="3"></citation> | Bit-perfect / resampling / DAC pipeline checks |
| AudioCheck high-definition test files | Uncompressed WAV test tones/noise at 88.2, 96, 176.4, and 192 kHz. <citation src="4"></citation> | High-sample-rate processing and analyzer validation |
| QOA test samples | Many lossless-source samples converted from real music and SQAM; mostly 44.1 kHz stereo, with a lot of content variety. <citation src="5"></citation> | Codec robustness and content-diversity testing |
#>

$OutputDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDir)
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

# Primary source: Espressif direct sample URLs.
# Use the short Galway samples for the default matrix so decode probes stay fast.
$shortStereoBase = "https://dl.espressif.com/dl/audio/gs-16b-2c-44100hz"
$shortMonoBase   = "https://dl.espressif.com/dl/audio/gs-16b-1c-44100hz"
$mp3RateStereoBase = "https://dl.espressif.com/dl/audio/ff-16b-2c"
$mp3RateMonoBase   = "https://dl.espressif.com/dl/audio/ff-16b-1c"

$files = @(
  @{ Name = "mp3-44100-stereo.mp3";             Url = "$shortStereoBase.mp3"  }
  @{ Name = "mp3-44100-mono.mp3";               Url = "$shortMonoBase.mp3"    }
  @{ Name = "mp3-08000-stereo.mp3";             Url = "$mp3RateStereoBase-8000hz.mp3"  }
  @{ Name = "mp3-08000-mono.mp3";               Url = "$mp3RateMonoBase-8000hz.mp3"    }
  @{ Name = "mp3-16000-stereo.mp3";             Url = "$mp3RateStereoBase-16000hz.mp3" }
  @{ Name = "mp3-16000-mono.mp3";               Url = "$mp3RateMonoBase-16000hz.mp3"   }
  @{ Name = "mp3-32000-stereo.mp3";             Url = "$mp3RateStereoBase-32000hz.mp3" }
  @{ Name = "mp3-32000-mono.mp3";               Url = "$mp3RateMonoBase-32000hz.mp3"   }
  @{ Name = "flac-44100-stereo.flac";           Url = "$shortStereoBase.flac" }
  @{ Name = "flac-44100-mono.flac";             Url = "$shortMonoBase.flac"   }
  @{ Name = "ogg-vorbis-44100-stereo.ogg";      Url = "$shortStereoBase.ogg"  }
  @{ Name = "ogg-vorbis-44100-mono.ogg";        Url = "$shortMonoBase.ogg"    }
  @{ Name = "opus-44100-stereo.opus";           Url = "$shortStereoBase.opus" }
  @{ Name = "opus-44100-mono.opus";             Url = "$shortMonoBase.opus"   }
  @{ Name = "wav-16bit-44100-stereo.wav";       Url = "$shortStereoBase.wav"  }
  @{ Name = "wav-16bit-44100-mono.wav";         Url = "$shortMonoBase.wav"    }
  @{ Name = "aiff-16bit-44100-stereo.aiff";     Url = "$shortStereoBase.aiff" }
  @{ Name = "aiff-16bit-44100-mono.aiff";       Url = "$shortMonoBase.aiff"   }
  @{ Name = "aac-adts-44100-stereo.aac";        Url = "$shortStereoBase.aac"  }
  @{ Name = "aac-adts-44100-mono.aac";          Url = "$shortMonoBase.aac"    }
  @{ Name = "aac-m4a-44100-stereo.m4a";         Url = "$shortStereoBase.m4a"  }
  @{ Name = "aac-m4a-44100-mono.m4a";           Url = "$shortMonoBase.m4a"    }
  @{ Name = "aac-mp4-44100-stereo.mp4";         Url = "$shortStereoBase.mp4"  }
)

# Optional high-res WAVs for 96 kHz coverage
if ($IncludeHighResWav) {
  $files += @(
    @{ Name = "wav-16bit-96000-stereo.wav"; Url = "https://www.audiocheck.net/download.php?filename=testtones/96khz_whitenoise.wav" }
  )
}

function Test-FixtureUrl {
  param(
    [Parameter(Mandatory)] [string]$Url,
    [Parameter(Mandatory)] [string]$Name
  )

  try {
    $null = Invoke-WebRequest -Uri $Url -Method Head -MaximumRedirection 5
    Write-Host "OK  $Name"
    return $true
  }
  catch {
    Write-Host "ERR $Name - $($_.Exception.Message)"
    return $false
  }
}

function Save-FixtureFile {
  param(
    [Parameter(Mandatory)] [string]$Url,
    [Parameter(Mandatory)] [string]$Path
  )

  if (Test-Path $Path) {
    Write-Host "Skip $([IO.Path]::GetFileName($Path))"
    return
  }

  Write-Host "Get $([IO.Path]::GetFileName($Path))"
  Invoke-WebRequest -Uri $Url -OutFile $Path
}

if ($ValidateUrls) {
  $failed = 0

  foreach ($f in $files) {
    if (-not (Test-FixtureUrl -Url $f.Url -Name $f.Name)) {
      $failed++
    }
  }

  if ($failed -ne 0) {
    throw "$failed fixture URL(s) failed validation."
  }
}

if ($ListOnly) {
  foreach ($f in $files) {
    Write-Host "$($f.Name) $($f.Url)"
  }

  Write-Host "Count: $($files.Count)"
  return
}

foreach ($f in $files) {
  $dest = Join-Path $OutputDir $f.Name
  Save-FixtureFile -Url $f.Url -Path $dest
}

Write-Host "Done."
Write-Host "Output: $OutputDir"
