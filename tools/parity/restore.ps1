<#
.SYNOPSIS
  Puts watch positions back from a backup.ps1 folder and checks them.
.DESCRIPTION
  Portal running: every history entry whose position or duration differs from the backup is
  written back with POST /api/progress (flush), then GET /api/history is checked against the
  backup. Entries are matched by id, season and episode; order and timestamps are not compared.
  Portal stopped (nothing answers and no portal.exe runs): library.json is copied back.
  Exit code 0 when everything matches or the file was copied back, 1 otherwise.
.EXAMPLE
  pwsh tools/parity/restore.ps1 -Backup <folder printed by backup.ps1>
#>
param(
    [Parameter(Mandatory)][string]$Backup,
    [string]$Server
)
$ErrorActionPreference = 'Stop'
if (-not $Server) {
    $saved = Join-Path $Backup 'backup.json'
    $Server = if (Test-Path $saved) { (Get-Content $saved -Raw | ConvertFrom-Json).server } else { 'http://127.0.0.1:8490' }
}
$library = Join-Path $env:LOCALAPPDATA 'Portal\library.json'

function Get-Key($e) { "$($e.id)|$($e.season)|$($e.episode)" }
function Test-Same($a, $b) { [math]::Abs([double]$a.position - [double]$b.position) -lt 0.001 -and [math]::Abs([double]$a.duration - [double]$b.duration) -lt 0.001 }
function Copy-Library {
    Copy-Item (Join-Path $Backup 'library.json') $library -Force
    Write-Host "Portal is stopped: copied library.json back to $library"
    exit 0
}

$running = $true
try { $now = Invoke-RestMethod "$Server/api/history" -TimeoutSec 10 } catch { $running = $false }
if (-not $running) {
    if (Get-Process portal -ErrorAction SilentlyContinue) {
        Write-Error "$Server does not answer but portal.exe is running; stop it (portal-launcher.exe --stop) or pass -Server"
    }
    Copy-Library
}

$historyFile = Join-Path $Backup 'history.json'
if (-not (Test-Path $historyFile)) {
    Write-Error "No history.json in $Backup; stop Portal and run this again to copy library.json back"
}
$want = (Get-Content $historyFile -Raw | ConvertFrom-Json).items
$byKey = @{}; foreach ($e in $now.items) { $byKey[(Get-Key $e)] = $e }

foreach ($e in $want) {
    $cur = $byKey[(Get-Key $e)]
    if ($cur -and (Test-Same $cur $e)) { continue }
    $body = [ordered]@{ id = $e.id; kind = $e.kind; name = $e.name; art = $e.art; season = $e.season
        episode = $e.episode; file = $e.file; position = $e.position; duration = $e.duration; flush = $true }
    Invoke-RestMethod "$Server/api/progress" -Method Post -ContentType 'application/json' -Body ($body | ConvertTo-Json) | Out-Null
    Write-Host "restored $($e.name) S$($e.season)E$($e.episode) to $($e.position)s"
}

$after = @{}; foreach ($e in (Invoke-RestMethod "$Server/api/history").items) { $after[(Get-Key $e)] = $e }
$diffs = @()
foreach ($e in $want) {
    $cur = $after[(Get-Key $e)]
    if (-not $cur) { $diffs += "missing: $($e.name) S$($e.season)E$($e.episode)" }
    elseif (-not (Test-Same $cur $e)) { $diffs += "differs: $($e.name) S$($e.season)E$($e.episode) $($cur.position)/$($cur.duration) want $($e.position)/$($e.duration)" }
}
$extra = @($after.Keys | Where-Object { $k = $_; -not ($want | Where-Object { (Get-Key $_) -eq $k }) })
foreach ($k in $extra) { $diffs += "new entry: $($after[$k].name) S$($after[$k].season)E$($after[$k].episode)" }

if ($diffs.Count -eq 0) { Write-Host "History matches the backup ($($want.Count) entries)"; exit 0 }
$diffs | ForEach-Object { Write-Warning $_ }
Write-Warning "History still differs. Stop Portal (portal-launcher.exe --stop) and run this again: it then copies library.json back."
exit 1
