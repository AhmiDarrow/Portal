<#
.SYNOPSIS
  Backs up Portal's settings and watch positions before a parity run.
.DESCRIPTION
  Copies library.json, prefs.json and cast.json from %LOCALAPPDATA%\Portal into a new
  timestamped folder, and saves GET /api/history there as history.json (the positions
  restore.ps1 puts back). Prints the folder on the last line.
.EXAMPLE
  pwsh tools/parity/backup.ps1
  pwsh tools/parity/backup.ps1 -Server http://127.0.0.1:8490 -Root D:\portal-backups
#>
param(
    [string]$Server = "http://127.0.0.1:$(if ($env:PORTAL_PORT) { $env:PORTAL_PORT } else { 8490 })",
    [string]$Root = $(if ($env:PORTAL_BACKUP_ROOT) { $env:PORTAL_BACKUP_ROOT } else { Join-Path $env:TEMP 'portal-backups' })
)
$ErrorActionPreference = 'Stop'
$data = Join-Path $env:LOCALAPPDATA 'Portal'
$dir = Join-Path $Root (Get-Date -Format 'yyyyMMdd-HHmmss')
New-Item -ItemType Directory -Force $dir | Out-Null

foreach ($name in 'library.json', 'prefs.json', 'cast.json') {
    $src = Join-Path $data $name
    if (Test-Path $src) { Copy-Item $src (Join-Path $dir $name) }
    else { Write-Warning "$name not found in $data; skipped" }
}

try {
    $history = Invoke-RestMethod "$Server/api/history" -TimeoutSec 10
    $history | ConvertTo-Json -Depth 10 | Set-Content -Encoding utf8 (Join-Path $dir 'history.json')
    Write-Host "$($history.items.Count) history positions saved from $Server"
} catch {
    # The library file still holds every position; restore.ps1 falls back to it.
    Write-Warning "GET $Server/api/history failed ($($_.Exception.Message)); only the files were saved"
}
@{ server = $Server; at = (Get-Date).ToString('o') } | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $dir 'backup.json')
Write-Output $dir
