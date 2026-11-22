<#
.SYNOPSIS
Builds Portal's RemedyUI dashboard (crates/web-app) into a page Portal serves at /next/.

.DESCRIPTION
Runs `cargo remedy package --target web` for crates/web-app, using cargo-remedy from the
RemedyUI checkout beside this repo. The page (index.html, app.js, app_bg.wasm, manifest,
icons, service worker) lands in target\remedy\web\portal-web, which portal.exe serves at
/next/ when run from this checkout; an installed Portal serves its web-next folder instead
(or whatever PORTAL_WEB_NEXT_DIST names).

Needs the wasm32-unknown-unknown target and the wasm-bindgen CLI of the version Cargo.lock
has (cargo remedy says which when they are missing).

.PARAMETER Out
Where the page goes, instead of target\remedy\web\portal-web.

.PARAMETER Dev
A debug build: quicker to make, larger and slower.

.PARAMETER RemedyUI
The RemedyUI checkout that has crates\cargo-remedy (default: ..\remedyui-main beside
this repo's folder).

.PARAMETER Jobs
Parallel compile jobs (default 2, to keep the PC responsive).
#>
param(
    [string]$Out = "",
    [switch]$Dev,
    [string]$RemedyUI = "",
    [int]$Jobs = 2
)
$ErrorActionPreference = "Stop"

$repo = Resolve-Path (Join-Path $PSScriptRoot "..")
if (-not $RemedyUI) { $RemedyUI = Join-Path (Split-Path $repo -Parent) "remedyui-main" }
# A worktree under pw\<name> has the RemedyUI checkout two folders up.
if (-not (Test-Path (Join-Path $RemedyUI "crates\cargo-remedy\Cargo.toml"))) {
    $RemedyUI = Join-Path (Split-Path (Split-Path $repo -Parent) -Parent) "remedyui-main"
}
$tool = Join-Path $RemedyUI "crates\cargo-remedy\Cargo.toml"
if (-not (Test-Path $tool)) { throw "cargo-remedy not found at $tool (pass -RemedyUI <checkout>)" }

$env:CARGO_INCREMENTAL = "0"
$env:CARGO_BUILD_JOBS = "$Jobs"

$remedyArgs = @("remedy", "package", "--target", "web")
if ($Dev) { $remedyArgs += "--dev" }
if ($Out) { $remedyArgs += @("--out", $Out) }

Push-Location (Join-Path $repo "crates\web-app")
try {
    # cargo-remedy itself builds into this repo's target folder, leaving the RemedyUI
    # checkout untouched.
    & cargo run --quiet --release -j $Jobs --manifest-path $tool --target-dir (Join-Path $repo "target\cargo-remedy") -- @remedyArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo remedy package failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}

$page = if ($Out) { $Out } else { Join-Path $repo "target\remedy\web\portal-web" }
Write-Host "Portal's RemedyUI page: $page (served at /next/)"
