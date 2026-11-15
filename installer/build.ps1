<#
.SYNOPSIS
  Builds the Portal installer: target\installer\Portal-Setup-<version>.exe

.DESCRIPTION
  The installer carries only Portal's own files. Everything else is
  downloaded on the user's PC during setup, from pinned URLs that are checked
  against SHA-256 hashes resolved here at build time:

    ffmpeg   gyan.dev "essentials" build (GPL; runs as a separate program)
    Kodi     the current stable release, only when Kodi is not installed
    Python   the embeddable runtime plus the cast helper's wheels, only when
             the "Cast to TVs" option is selected

  Steps: build portal.exe, portal-launcher.exe and web\dist; stage them;
  resolve the downloads into target\installer\downloads.iss; compile
  installer\portal.iss with Inno Setup 6; write the installer's SHA-256 to
  Portal-Setup-<version>.exe.sha256 for Portal's updater.

  Requires Rust, Node.js, Python 3.13 with pip, and Inno Setup 6.

.PARAMETER SkipBuild
  Reuse the existing release binaries and web\dist.
#>
[CmdletBinding()]
param(
  [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Root = Split-Path -Parent $PSScriptRoot
$Out = Join-Path $Root 'target\installer'
$Cache = Join-Path $Out 'cache'
$Stage = Join-Path $Out 'stage'

# The cast helper runs on this Python. Wheels are resolved for the same minor version.
$PythonVersion = '3.13.13'
$KodiMirror = 'https://mirrors.kodi.tv/releases/windows/win64/'

function Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }

function Invoke-Checked {
  param([string]$File, [string[]]$Arguments, [string]$WorkDir = $Root)
  Push-Location $WorkDir
  try {
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$File $($Arguments -join ' ') failed with exit code $LASTEXITCODE" }
  } finally { Pop-Location }
}

function Get-Text([string]$Url) {
  $content = (Invoke-WebRequest -Uri $Url -UseBasicParsing).Content
  # Servers that label text as application/octet-stream come back as bytes.
  if ($content -is [byte[]]) { $content = [System.Text.Encoding]::UTF8.GetString($content) }
  [string]$content
}

function Get-RemoteSize([string]$Url) {
  $response = Invoke-WebRequest -Uri $Url -Method Head -UseBasicParsing
  [int64]($response.Headers['Content-Length'] | Select-Object -First 1)
}

# Downloads into the build cache (never into the installer) to hash or check a file.
function Get-Cached([string]$Url, [string]$Name) {
  $path = Join-Path $Cache $Name
  if (-not (Test-Path $path)) {
    Write-Host "    downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile "$path.part" -UseBasicParsing
    Move-Item "$path.part" $path -Force
  }
  $path
}

function Get-Sha256([string]$Path) { (Get-FileHash $Path -Algorithm SHA256).Hash.ToLower() }

function Find-Iscc {
  $candidates = @(
    (Get-Command iscc -ErrorAction SilentlyContinue).Source,
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "$env:USERPROFILE\scoop\apps\inno-setup\current\ISCC.exe"
  )
  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path $candidate)) { return $candidate }
  }
  throw 'Inno Setup 6 was not found. Install it (winget install JRSoftware.InnoSetup, or scoop install inno-setup).'
}

function Iss([string]$Text) { $Text.Replace('"', '""') }

New-Item -ItemType Directory -Force $Cache | Out-Null

$Version = (Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' |
  Select-Object -First 1).Matches[0].Groups[1].Value

if (-not $SkipBuild) {
  Step "Building Portal $Version"
  Invoke-Checked cargo @('build', '--release', '--locked', '-p', 'portal-http', '-p', 'portal-launcher')
  $web = Join-Path $Root 'web'
  Invoke-Checked npm @('ci', '--no-audit', '--no-fund') $web
  Invoke-Checked npm @('run', 'build') $web
}

Step 'Staging Portal'
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
New-Item -ItemType Directory -Force $Stage, (Join-Path $Stage 'vendor'), (Join-Path $Stage 'cast') | Out-Null
Copy-Item (Join-Path $Root 'target\release\portal.exe') $Stage
Copy-Item (Join-Path $Root 'target\release\portal-launcher.exe') $Stage
Copy-Item (Join-Path $Root 'web\dist') (Join-Path $Stage 'web') -Recurse
Copy-Item (Join-Path $Root 'web\vendor\hls.min.js') (Join-Path $Stage 'vendor')
Copy-Item (Join-Path $Root 'web\vendor\hls.js.LICENSE.txt') (Join-Path $Stage 'vendor')
Copy-Item (Join-Path $Root 'tools\cast\cast_agent.py') (Join-Path $Stage 'cast')
foreach ($doc in 'LICENSE', 'README.md') { Copy-Item (Join-Path $Root $doc) $Stage }
# The licences of the Rust crates built into portal.exe and portal-launcher.exe.
Invoke-Checked python @((Join-Path $Root 'tools\notices.py'), (Join-Path $Stage 'THIRD-PARTY-NOTICES.txt'))
foreach ($required in 'portal.exe', 'portal-launcher.exe', 'web\index.html', 'vendor\hls.min.js', 'cast\cast_agent.py', 'THIRD-PARTY-NOTICES.txt') {
  if (-not (Test-Path (Join-Path $Stage $required))) { throw "Staged install is missing $required" }
}

Step 'Resolving ffmpeg'
$ffmpegVersion = (Get-Text 'https://www.gyan.dev/ffmpeg/builds/release-version').Trim()
$ffmpegName = "ffmpeg-$ffmpegVersion-essentials_build.7z"
$ffmpegUrl = "https://github.com/GyanD/codexffmpeg/releases/download/$ffmpegVersion/$ffmpegName"
$ffmpegPublished = (Get-Text 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.7z.sha256').Trim().Split()[0].ToLower()
$ffmpegFile = Get-Cached $ffmpegUrl $ffmpegName
$ffmpegSha = Get-Sha256 $ffmpegFile
if ($ffmpegSha -ne $ffmpegPublished) { throw "ffmpeg $ffmpegVersion does not match gyan.dev's published hash" }
$ffmpegSize = (Get-Item $ffmpegFile).Length
Write-Host "    ffmpeg $ffmpegVersion ($([math]::Round($ffmpegSize / 1MB)) MB)"

Step 'Resolving Kodi'
$kodiListing = Get-Text $KodiMirror
$kodiFile = [regex]::Matches($kodiListing, 'kodi-(\d+)\.(\d+)-([A-Za-z]+)-x64\.exe') |
  Sort-Object { [int]$_.Groups[1].Value }, { [int]$_.Groups[2].Value } |
  Select-Object -Last 1
if (-not $kodiFile) { throw 'No Kodi release found on the mirror' }
$kodiVersion = "$($kodiFile.Groups[1].Value).$($kodiFile.Groups[2].Value) $($kodiFile.Groups[3].Value)"
$kodiUrl = "$KodiMirror$($kodiFile.Value)"
$kodiSha = (Get-Text "$kodiUrl.sha256").Trim().Split()[0].ToLower()
$kodiSize = Get-RemoteSize $kodiUrl
Write-Host "    Kodi $kodiVersion ($([math]::Round($kodiSize / 1MB)) MB)"

Step 'Resolving the cast runtime'
$pythonUrl = "https://www.python.org/ftp/python/$PythonVersion/python-$PythonVersion-embed-amd64.zip"
$pythonFile = Get-Cached $pythonUrl "python-$PythonVersion-embed-amd64.zip"
$pythonSha = Get-Sha256 $pythonFile
$castBytes = (Get-Item $pythonFile).Length
$report = Join-Path $Out 'cast-wheels.json'
$pythonMinor = ($PythonVersion.Split('.')[0..1] -join '.')
Invoke-Checked python @(
  '-m', 'pip', 'install', '--dry-run', '--quiet', '--disable-pip-version-check', '--ignore-installed',
  '--report', $report, '--target', (Join-Path $Out 'cast-dry-run'),
  '--only-binary=:all:', '--platform', 'win_amd64', '--implementation', 'cp',
  '--python-version', $pythonMinor,
  '-r', (Join-Path $Root 'tools\cast\requirements.txt')
)
$wheels = (Get-Content $report -Raw -Encoding utf8 | ConvertFrom-Json).install
$wheelSpec = @()
$runtimeId = @("python-$PythonVersion")
foreach ($wheel in $wheels) {
  $info = $wheel.download_info
  $sha = $info.archive_info.hashes.sha256
  if (-not $sha) { throw "No sha256 for $($info.url)" }
  $wheelSpec += "$($info.url)|$sha"
  $runtimeId += "$($wheel.metadata.name)-$($wheel.metadata.version)"
  $castBytes += Get-RemoteSize $info.url
}
Write-Host "    $($wheels.Count) packages, $([math]::Round($castBytes / 1MB)) MB with Python"

Step 'Writing downloads.iss'
$downloads = @"
; Generated by installer\build.ps1. Do not edit.
#define FfmpegVersion "$(Iss $ffmpegVersion)"
#define FfmpegUrl "$(Iss $ffmpegUrl)"
#define FfmpegSha256 "$ffmpegSha"
#define FfmpegMB "$([math]::Ceiling($ffmpegSize / 1MB))"
#define KodiVersion "$(Iss $kodiVersion)"
#define KodiUrl "$(Iss $kodiUrl)"
#define KodiSha256 "$kodiSha"
#define KodiMB "$([math]::Ceiling($kodiSize / 1MB))"
#define PythonUrl "$(Iss $pythonUrl)"
#define PythonSha256 "$pythonSha"
#define CastWheels "$(Iss ($wheelSpec -join '|'))"
#define CastRuntimeId "$(Iss ($runtimeId -join ';'))"
#define CastMB "$([math]::Ceiling($castBytes / 1MB))"
"@
Set-Content (Join-Path $Out 'downloads.iss') $downloads -Encoding utf8

Step 'Compiling the installer'
Invoke-Checked (Find-Iscc) @(
  '/Q',
  "/DAppVersion=$Version",
  "/DStageDir=$Stage",
  "/DRepoDir=$Root",
  "/DDownloads=$(Join-Path $Out 'downloads.iss')",
  "/O$Out",
  (Join-Path $PSScriptRoot 'portal.iss')
)

$setup = Join-Path $Out "Portal-Setup-$Version.exe"
# Portal's updater checks the download against this (or GitHub's asset digest).
# Upload it to the release next to the installer.
$setupName = Split-Path -Leaf $setup
[System.IO.File]::WriteAllText("$setup.sha256", "$(Get-Sha256 $setup)  $setupName`n")
Write-Host ''
Write-Host "Installer: $setup ($([math]::Round((Get-Item $setup).Length / 1MB, 1)) MB)" -ForegroundColor Green
Write-Host "Checksum:  $setup.sha256" -ForegroundColor Green
