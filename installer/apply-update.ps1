<#
.SYNOPSIS
  Installs the Portal setup that sits in this folder.

.DESCRIPTION
  Checks Portal-Setup-<version>.exe against the .sha256 file beside it, then
  runs the same silent update Portal uses. Library, watchlist, and settings
  stay. Portal closes and comes back in the tray.

  Copy this script, apply-update.cmd, the setup exe, and the .sha256
  file into one folder. Double-click the cmd file.
#>
[CmdletBinding()]
param(
  [string]$Setup
)

$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Setup) {
  # Newest version last. By name, 0.2.10 would sort before 0.2.9.
  $found = @(Get-ChildItem -Path $here -Filter 'Portal-Setup-*.exe' -File | Sort-Object {
    $version = $null
    if (-not [version]::TryParse(($_.BaseName -replace '^Portal-Setup-', ''), [ref]$version)) { $version = [version]'0.0' }
    $version
  }, Name)
  if ($found.Count -eq 0) {
    throw "No Portal-Setup exe is in $here. Keep the installer in the same folder as this script."
  }
  $Setup = $found[-1].FullName
}
if (-not (Test-Path -LiteralPath $Setup)) { throw "Installer not found: $Setup" }

$sumPath = "$Setup.sha256"
if (-not (Test-Path -LiteralPath $sumPath)) { throw "Checksum not found: $sumPath" }
$expected = ((Get-Content -LiteralPath $sumPath -Raw).Trim() -split '\s+')[0].ToLower()
if ($expected -notmatch '^[0-9a-f]{64}$') { throw "The checksum file is not a SHA-256 hash." }

Write-Host "Checking $(Split-Path -Leaf $Setup)"
$actual = (Get-FileHash -LiteralPath $Setup -Algorithm SHA256).Hash.ToLower()
if ($actual -ne $expected) { throw "The installer does not match its checksum. Do not run it." }

Write-Host "Installing. Portal will close and open again in the tray."
# WMI starts setup outside this window's process job. Waiting on that process
# returns when setup exits, and Portal keeps running after this window closes.
$command = '"{0}" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /UPDATE=1' -f $Setup
$started = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $command }
if ($started.ReturnValue -ne 0 -or -not $started.ProcessId) {
  throw "Could not start the installer (code $($started.ReturnValue))."
}
$proc = Get-Process -Id $started.ProcessId
# Holding the handle keeps the exit code readable; without it ExitCode is empty.
$null = $proc.Handle
$proc.WaitForExit()
if ($proc.ExitCode -ne 0) { throw "The installer stopped with code $($proc.ExitCode)." }
Write-Host "Portal is updated."
