//! Updates from the latest GitHub release.
//!
//! Portal checks `https://api.github.com/repos/{repo}/releases/latest` about
//! 30 seconds after it starts and then every 12 hours, and only reports what
//! it finds. Nothing is downloaded until the launcher (or the dashboard) posts
//! `/api/update/install`. The installer `Portal-Setup-<version>.exe` is then
//! downloaded to `%LOCALAPPDATA%\Portal\updates`, checked against the SHA-256
//! GitHub publishes for the asset (or a sibling `<name>.sha256` asset), and
//! started silently. The installer stops Portal and starts it again.
//!
//! `PORTAL_UPDATE_REPO` (owner/name) picks another repository and
//! `PORTAL_NO_UPDATE=1` turns the automatic checks off.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use portal_api::{UpdateInfo, UpdateState};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{fail, launcher_only, ApiError, AppState};

const DEFAULT_REPO: &str = "AhmiDarrow/Portal";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);
const CHECK_LIMIT: Duration = Duration::from_secs(20);
/// A check this recent answers the next ask too. Anyone on the network may
/// ask, and GitHub allows 60 calls an hour without a token; past that the
/// automatic checks fail as well.
const CHECK_REUSE: Duration = Duration::from_secs(60);
const STALL_LIMIT: Duration = Duration::from_secs(60);
/// The whole installer download, request included. The installer is about
/// 6 MB, so this allows a line slower than 100 kbit/s.
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(10 * 60);

const RELEASE_CAP: usize = 4 << 20;
const SUM_CAP: usize = 4096;
const SETUP_CAP: u64 = 200 << 20;

const INSTALLER_ARGS: [&str; 4] = [
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/UPDATE=1",
];

const CHECK_FAILED: &str = "Could not check for updates.";
const DOWNLOAD_FAILED: &str = "Could not download the update.";
const DOWNLOAD_TOO_SLOW: &str =
    "The update download took longer than 10 minutes, so it was stopped. Try again later.";
const NO_CHECKSUM: &str = "The update is not signed with a checksum.";
const BAD_CHECKSUM: &str = "The update did not match its checksum.";
const LAUNCH_FAILED: &str = "Could not start the installer.";
const INSTALL_FAILED: &str = "The installer did not finish.";
const UP_TO_DATE: &str = "Portal is up to date.";
const NO_INSTALLER: &str = "This release has no Windows installer.";

/// A release version: up to three numeric parts, missing parts are zero.
/// Prereleases (`1.2.0-rc.1`) are not versions Portal updates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version([u64; 3]);

impl Version {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let core = text.split('+').next().unwrap_or_default();
        if core.contains('-') {
            return None;
        }
        let mut parts = [0u64; 3];
        for (index, piece) in core.split('.').enumerate() {
            if index == parts.len()
                || piece.is_empty()
                || piece.len() > 9
                || !piece.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            parts[index] = piece.parse().ok()?;
        }
        Some(Self(parts))
    }

    fn current() -> Self {
        Self::parse(CURRENT).unwrap_or(Self([0, 0, 0]))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [major, minor, patch] = self.0;
        write!(f, "{major}.{minor}.{patch}")
    }
}

/// Where an asset downloads from: its `browser_download_url`.
#[derive(Debug, Clone, PartialEq)]
struct AssetLink {
    browser: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Checksum {
    Known([u8; 32]),
    /// A `<name>.sha256` asset.
    File(AssetLink),
    Missing,
}

#[derive(Debug, Clone, PartialEq)]
struct Setup {
    name: String,
    link: AssetLink,
    size: u64,
    checksum: Checksum,
}

#[derive(Debug, Clone, PartialEq)]
struct Release {
    version: Version,
    page: Option<String>,
    setup: Option<Setup>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// Reads a `releases/latest` body. `Ok(None)` when the release is not one
/// Portal can update to (draft, prerelease, or a tag that is not a version).
fn parse_release(body: &[u8]) -> Result<Option<Release>, serde_json::Error> {
    let release: GhRelease = serde_json::from_slice(body)?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::parse(&release.tag_name) else {
        tracing::debug!("latest release tag is not a version");
        return Ok(None);
    };
    let tag = release.tag_name.trim();
    let tag = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    let page = Some(release.html_url)
        .filter(|url| url.starts_with("https://github.com/") && url.len() <= 512);
    Ok(Some(Release {
        version,
        page,
        setup: pick_setup(&release.assets, &[tag.to_string(), version.to_string()]),
    }))
}

fn pick_setup(assets: &[GhAsset], versions: &[String]) -> Option<Setup> {
    let names: Vec<String> = versions
        .iter()
        .map(|version| format!("portal-setup-{}.exe", version.to_ascii_lowercase()))
        .collect();
    let asset = assets.iter().find(|asset| {
        names.contains(&asset.name.to_ascii_lowercase()) && https_url(&asset.browser_download_url)
    })?;
    let checksum = if let Some(hash) = asset.digest.as_deref().and_then(parse_digest) {
        Checksum::Known(hash)
    } else {
        let sum_name = format!("{}.sha256", asset.name).to_ascii_lowercase();
        assets
            .iter()
            .find(|other| {
                other.name.to_ascii_lowercase() == sum_name
                    && https_url(&other.browser_download_url)
            })
            .map(|other| Checksum::File(asset_link(other)))
            .unwrap_or(Checksum::Missing)
    };
    Some(Setup {
        name: asset.name.clone(),
        link: asset_link(asset),
        size: asset.size,
        checksum,
    })
}

fn asset_link(asset: &GhAsset) -> AssetLink {
    AssetLink {
        browser: asset.browser_download_url.clone(),
    }
}

fn https_url(url: &str) -> bool {
    url.starts_with("https://") && url.len() <= 2048
}

fn asset_request(client: &reqwest::Client, link: &AssetLink) -> reqwest::RequestBuilder {
    client.get(&link.browser)
}

fn build_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("Portal/{CURRENT}"))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            // Tests redirect between two loopback ports over plain HTTP.
            let secure =
                url.scheme() == "https" || (cfg!(test) && url.host_str() == Some("127.0.0.1"));
            if attempt.previous().len() >= 5 || !secure {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .ok()
}

fn parse_hex32(text: &str) -> Option<[u8; 32]> {
    let text = text.as_bytes();
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, pair) in text.chunks(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out[index] = (high * 16 + low) as u8;
    }
    Some(out)
}

/// GitHub's asset `digest` field: `sha256:<hex>`.
fn parse_digest(text: &str) -> Option<[u8; 32]> {
    let (kind, hex) = text.trim().split_once(':')?;
    if !kind.eq_ignore_ascii_case("sha256") {
        return None;
    }
    parse_hex32(hex)
}

/// A `sha256sum` line, `<hex>  <name>` (or `<hex> *<name>`, or the hash
/// alone). A named line must name the installer.
fn parse_sum_file(body: &str, name: &str) -> Option<[u8; 32]> {
    let line = body.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut parts = line.splitn(2, char::is_whitespace);
    let hash = parse_hex32(parts.next()?)?;
    if let Some(rest) = parts.next() {
        let listed = rest.trim().trim_start_matches('*');
        if !listed.is_empty() && !listed.eq_ignore_ascii_case(name) {
            return None;
        }
    }
    Some(hash)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn repo_from_env() -> String {
    let Ok(raw) = std::env::var("PORTAL_UPDATE_REPO") else {
        return DEFAULT_REPO.to_string();
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return DEFAULT_REPO.to_string();
    }
    if repo_ok(raw) {
        raw.to_string()
    } else {
        tracing::warn!("PORTAL_UPDATE_REPO is not owner/name; using {DEFAULT_REPO}");
        DEFAULT_REPO.to_string()
    }
}

fn repo_ok(repo: &str) -> bool {
    let Some((owner, name)) = repo.split_once('/') else {
        return false;
    };
    let part_ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    part_ok(owner) && part_ok(name)
}

fn automatic_from_env() -> bool {
    !matches!(
        std::env::var("PORTAL_NO_UPDATE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

struct Shared {
    state: UpdateState,
    release: Option<Release>,
    progress: Option<f64>,
    error: Option<String>,
    /// A downloaded installer whose checksum matched, for this version.
    ready: Option<(Version, PathBuf)>,
    /// When GitHub last answered a check.
    checked: Option<Instant>,
}

pub struct Updater {
    repo: String,
    automatic: bool,
    dir: PathBuf,
    client: Option<reqwest::Client>,
    download_limit: Duration,
    shared: Mutex<Shared>,
    /// One check or install at a time.
    work: tokio::sync::Mutex<()>,
}

type Refusal = (StatusCode, &'static str);

impl Updater {
    pub fn new(portal_dir: &Path) -> Self {
        let dir = portal_dir.join("updates");
        clear_stale(&dir);
        Self {
            repo: repo_from_env(),
            automatic: automatic_from_env(),
            dir,
            client: build_client(),
            download_limit: DOWNLOAD_LIMIT,
            shared: Mutex::new(Shared {
                state: UpdateState::Idle,
                release: None,
                progress: None,
                error: None,
                ready: None,
                checked: None,
            }),
            work: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn view(&self) -> UpdateInfo {
        view_of(&self.lock())
    }

    /// Checks now and returns the result. A check or install already under
    /// way is not repeated, nor one that GitHub answered within `CHECK_REUSE`.
    pub async fn check(&self) -> UpdateInfo {
        let guard = match self.work.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                if self.lock().state == UpdateState::Checking {
                    let _ = tokio::time::timeout(CHECK_LIMIT, self.work.lock()).await;
                }
                return self.view();
            }
        };
        if recently_checked(&self.lock()) {
            return self.view();
        }
        let _ = self.check_locked(false).await;
        drop(guard);
        self.view()
    }

    /// With `keep_known`, a failed check leaves the release already known in
    /// place and is not reported, so an install can go on with it.
    async fn check_locked(&self, keep_known: bool) -> Result<(), ()> {
        {
            let mut shared = self.lock();
            if shared.state == UpdateState::Installing {
                return Ok(());
            }
            shared.state = UpdateState::Checking;
            shared.progress = None;
            shared.error = None;
        }
        let fetched = tokio::time::timeout(CHECK_LIMIT, self.fetch_release())
            .await
            .unwrap_or_else(|_| {
                tracing::warn!("update check timed out");
                Err(())
            });
        let mut shared = self.lock();
        match fetched {
            Ok(release) => {
                shared.checked = Some(Instant::now());
                let current = Version::current();
                if let Some(found) = &release {
                    let known = shared.release.as_ref().map(|old| old.version);
                    if found.version > current && known != Some(found.version) {
                        if found.setup.is_some() {
                            tracing::info!("Portal {} is available", found.version);
                        } else {
                            tracing::info!("Portal {} has no installer asset", found.version);
                        }
                    }
                }
                if shared.ready.as_ref().map(|(version, _)| *version)
                    != release.as_ref().map(|found| found.version)
                {
                    shared.ready = None;
                }
                shared.error = release
                    .as_ref()
                    .filter(|found| found.version > current && found.setup.is_none())
                    .map(|found| format!("Portal {} has no Windows installer yet.", found.version));
                shared.release = release;
                shared.state = if shared.ready.is_some() {
                    UpdateState::Ready
                } else {
                    UpdateState::Idle
                };
                Ok(())
            }
            Err(()) if keep_known && shared.release.is_some() => {
                shared.state = if shared.ready.is_some() {
                    UpdateState::Ready
                } else {
                    UpdateState::Idle
                };
                Ok(())
            }
            Err(()) => {
                shared.state = UpdateState::Failed;
                shared.error = Some(CHECK_FAILED.to_string());
                Err(())
            }
        }
    }

    async fn fetch_release(&self) -> Result<Option<Release>, ()> {
        let client = self.client.as_ref().ok_or(())?;
        let url = format!("https://api.github.com/repos/{}/releases/latest", self.repo);
        let request = client
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        let response = request
            .send()
            .await
            .map_err(|err| tracing::warn!("update check failed: {}", err.without_url()))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            tracing::debug!("{} has no public release", self.repo);
            return Ok(None);
        }
        if !status.is_success() {
            tracing::warn!("update check failed: GitHub answered {}", status.as_u16());
            return Err(());
        }
        let body = capped_body(response, RELEASE_CAP).await?;
        parse_release(&body).map_err(|_| tracing::warn!("update check failed: unreadable release"))
    }

    /// Downloads and verifies the installer if needed, then starts it.
    /// Returns the started process so the caller can watch it.
    async fn install(&self) -> Result<(UpdateInfo, Option<std::process::Child>), Refusal> {
        let _guard = self.work.lock().await;
        if self.lock().state == UpdateState::Installing {
            return Ok((self.view(), None));
        }
        // Look again first: the last check can be 12 hours old, and since then
        // a newer release may be out or the installer uploaded again with a
        // new checksum (which would fail every retry until the next check).
        // When GitHub does not answer, the release already known is used.
        if self.check_locked(true).await.is_err() {
            return Err((StatusCode::BAD_GATEWAY, CHECK_FAILED));
        }
        let Some(release) = self.lock().release.clone() else {
            return Err((StatusCode::CONFLICT, UP_TO_DATE));
        };
        if release.version <= Version::current() {
            return Err((StatusCode::CONFLICT, UP_TO_DATE));
        }
        let Some(setup) = release.setup.clone() else {
            return Err((StatusCode::CONFLICT, NO_INSTALLER));
        };
        let target = self
            .dir
            .join(format!("Portal-Setup-{}.exe", release.version));

        // Always through fetch_installer: it re-hashes a file already downloaded
        // and uses it only if it still matches, so nothing replaced since is run.
        self.set_state(UpdateState::Downloading, Some(0.0), None);
        let installer = match self.fetch_installer(&setup, &target).await {
            Ok(()) => target,
            Err(refusal) => {
                self.set_state(UpdateState::Failed, None, Some(refusal.1));
                return Err(refusal);
            }
        };
        {
            let mut shared = self.lock();
            shared.ready = Some((release.version, installer.clone()));
            shared.state = UpdateState::Ready;
            shared.progress = None;
            shared.error = None;
        }

        let child = match launch(&installer, &self.dir) {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!("installer did not start: {err}");
                self.set_state(UpdateState::Failed, None, Some(LAUNCH_FAILED));
                return Err((StatusCode::INTERNAL_SERVER_ERROR, LAUNCH_FAILED));
            }
        };
        tracing::info!("started the installer for Portal {}", release.version);
        self.set_state(UpdateState::Installing, None, None);
        Ok((self.view(), Some(child)))
    }

    fn set_state(&self, state: UpdateState, progress: Option<f64>, error: Option<&str>) {
        let mut shared = self.lock();
        shared.state = state;
        shared.progress = progress;
        shared.error = error.map(str::to_string);
    }

    fn set_progress(&self, progress: Option<f64>) {
        self.lock().progress = progress.map(|value| value.clamp(0.0, 1.0));
    }

    async fn fetch_installer(&self, setup: &Setup, target: &Path) -> Result<(), Refusal> {
        let expected = match &setup.checksum {
            Checksum::Known(hash) => *hash,
            Checksum::Missing => return Err((StatusCode::CONFLICT, NO_CHECKSUM)),
            Checksum::File(link) => {
                let body = self
                    .get_small(link, SUM_CAP)
                    .await
                    .map_err(|()| (StatusCode::BAD_GATEWAY, DOWNLOAD_FAILED))?;
                let text = String::from_utf8_lossy(&body);
                parse_sum_file(&text, &setup.name).ok_or((StatusCode::CONFLICT, NO_CHECKSUM))?
            }
        };
        if let Some(found) = hash_file(target.to_path_buf()).await {
            if found == expected {
                tracing::info!("using the installer already downloaded");
                return Ok(());
            }
        }
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, DOWNLOAD_FAILED))?;
        let part = target.with_extension("exe.part");
        // A trickle never trips the stall limit, so the whole download has one.
        let download = self.download(setup, expected, &part);
        let result = match tokio::time::timeout(self.download_limit, download).await {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("update download took too long");
                Err((StatusCode::GATEWAY_TIMEOUT, DOWNLOAD_TOO_SLOW))
            }
        };
        if result.is_err() {
            let _ = tokio::fs::remove_file(&part).await;
            return result;
        }
        let _ = tokio::fs::remove_file(target).await;
        if tokio::fs::rename(&part, target).await.is_err() {
            let _ = tokio::fs::remove_file(&part).await;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, DOWNLOAD_FAILED));
        }
        Ok(())
    }

    async fn get_small(&self, link: &AssetLink, cap: usize) -> Result<Vec<u8>, ()> {
        let client = self.client.as_ref().ok_or(())?;
        let response = asset_request(client, link)
            .timeout(CHECK_LIMIT)
            .send()
            .await
            .map_err(|err| tracing::warn!("checksum download failed: {}", err.without_url()))?;
        if !response.status().is_success() {
            tracing::warn!("checksum download failed: {}", response.status().as_u16());
            return Err(());
        }
        capped_body(response, cap).await
    }

    async fn download(
        &self,
        setup: &Setup,
        expected: [u8; 32],
        part: &Path,
    ) -> Result<(), Refusal> {
        const FAILED: Refusal = (StatusCode::BAD_GATEWAY, DOWNLOAD_FAILED);
        let client = self.client.as_ref().ok_or(FAILED)?;
        let request = asset_request(client, &setup.link);
        let mut response = tokio::time::timeout(CHECK_LIMIT, request.send())
            .await
            .map_err(|_| FAILED)?
            .map_err(|err| {
                tracing::warn!("update download failed: {}", err.without_url());
                FAILED
            })?;
        if !response.status().is_success() {
            tracing::warn!("update download failed: {}", response.status().as_u16());
            return Err(FAILED);
        }
        let total = response
            .content_length()
            .filter(|len| *len > 0)
            .or(Some(setup.size).filter(|size| *size > 0));
        if total.is_some_and(|len| len > SETUP_CAP) {
            tracing::warn!("update download refused: larger than the cap");
            return Err(FAILED);
        }
        self.set_progress(total.map(|_| 0.0));
        let mut file = tokio::fs::File::create(part)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, DOWNLOAD_FAILED))?;
        let mut hasher = Sha256::new();
        let mut got: u64 = 0;
        loop {
            let chunk = match tokio::time::timeout(STALL_LIMIT, response.chunk()).await {
                Ok(Ok(Some(chunk))) => chunk,
                Ok(Ok(None)) => break,
                Ok(Err(err)) => {
                    tracing::warn!("update download failed: {}", err.without_url());
                    return Err(FAILED);
                }
                Err(_) => {
                    tracing::warn!("update download stalled");
                    return Err(FAILED);
                }
            };
            got = got.saturating_add(chunk.len() as u64);
            if got > SETUP_CAP {
                tracing::warn!("update download refused: larger than the cap");
                return Err(FAILED);
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, DOWNLOAD_FAILED))?;
            if let Some(total) = total {
                self.set_progress(Some(got as f64 / total as f64));
            }
        }
        file.flush()
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, DOWNLOAD_FAILED))?;
        drop(file);
        verify(&hasher.finalize().into(), &expected)
    }
}

fn verify(found: &[u8; 32], expected: &[u8; 32]) -> Result<(), Refusal> {
    if found == expected {
        Ok(())
    } else {
        tracing::warn!(
            "update checksum mismatch: expected {}, got {}",
            hex(expected),
            hex(found)
        );
        Err((StatusCode::BAD_GATEWAY, BAD_CHECKSUM))
    }
}

/// GitHub answered within `CHECK_REUSE` and nothing has failed since, so
/// asking again would only say the same.
fn recently_checked(shared: &Shared) -> bool {
    matches!(shared.state, UpdateState::Idle | UpdateState::Ready)
        && shared.checked.is_some_and(|at| at.elapsed() < CHECK_REUSE)
}

fn view_of(shared: &Shared) -> UpdateInfo {
    let release = shared.release.as_ref();
    let available =
        release.is_some_and(|found| found.version > Version::current() && found.setup.is_some());
    UpdateInfo {
        current: CURRENT.to_string(),
        latest: release.map(|found| found.version.to_string()),
        available,
        state: shared.state,
        progress: shared.progress,
        error: shared.error.clone(),
        release_url: release.and_then(|found| found.page.clone()),
    }
}

async fn capped_body(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, ()> {
    let mut out = Vec::new();
    loop {
        let chunk = match tokio::time::timeout(STALL_LIMIT, response.chunk()).await {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => return Ok(out),
            _ => return Err(()),
        };
        if out.len().saturating_add(chunk.len()) > cap {
            return Err(());
        }
        out.extend_from_slice(&chunk);
    }
}

/// SHA-256 of a file already on disk, or None when it is missing or too big.
async fn hash_file(path: PathBuf) -> Option<[u8; 32]> {
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path).ok()?;
        if file.metadata().ok()?.len() > SETUP_CAP {
            return None;
        }
        let mut hasher = Sha256::new();
        std::io::copy(&mut std::io::BufReader::new(file), &mut hasher).ok()?;
        Some(hasher.finalize().into())
    })
    .await
    .ok()
    .flatten()
}

/// Removes partial downloads and installers for this version or older.
fn clear_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let current = Version::current();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let stale = name.ends_with(".part")
            || name
                .strip_prefix("portal-setup-")
                .and_then(|rest| rest.strip_suffix(".exe"))
                .and_then(Version::parse)
                .is_some_and(|version| version <= current);
        if stale {
            // A running installer cannot be deleted; the next start tries again.
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Starts the installer outside Portal's job object: the installer stops
/// Portal, and a job member would be killed with it.
#[cfg(windows)]
fn launch(installer: &Path, dir: &Path) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    let spawn = |flags: u32| {
        Command::new(installer)
            .args(INSTALLER_ARGS)
            // Not the install folder, which the installer replaces.
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    spawn(base | CREATE_BREAKAWAY_FROM_JOB).inspect_err(|err| {
        tracing::warn!("installer could not leave Portal's job ({err})");
    })
}

#[cfg(not(windows))]
fn launch(_installer: &Path, _dir: &Path) -> std::io::Result<std::process::Child> {
    let _ = INSTALLER_ARGS;
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "updates install on Windows only",
    ))
}

/// First check shortly after start, then every 12 hours, until shutdown.
pub fn spawn_automatic(state: AppState) {
    if !state.0.update.automatic {
        tracing::info!("automatic update checks are off");
        return;
    }
    let mut stopping = state.0.stopping.subscribe();
    tokio::spawn(async move {
        let mut wait = FIRST_CHECK;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stopping.wait_for(|stopping| *stopping) => return,
            }
            let _ = state.0.update.check().await;
            wait = CHECK_EVERY;
        }
    });
}

pub async fn update_get(State(state): State<AppState>) -> Json<UpdateInfo> {
    Json(state.0.update.view())
}

pub async fn update_check(State(state): State<AppState>) -> Json<UpdateInfo> {
    Json(state.0.update.check().await)
}

/// Loopback and the launcher's header only, like `/api/shutdown`. The work
/// runs in its own task, so a dropped request does not cut a download short.
pub async fn update_install(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<UpdateInfo>, ApiError> {
    if !launcher_only(peer, &headers) {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "Updates are only installed from this PC.",
        ));
    }
    // The installer is watched inside the task too, so a request dropped
    // while it runs still leaves "installing" when the installer fails.
    let outcome = tokio::spawn(async move {
        let (info, child) = state.0.update.install().await?;
        if let Some(child) = child {
            watch_installer(state, child);
        }
        Ok::<_, Refusal>(info)
    })
    .await
    .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, INSTALL_FAILED))?;
    let info = outcome.map_err(|(code, detail)| fail(code, detail))?;
    Ok(Json(info))
}

/// The installer normally stops Portal. If it exits while Portal still runs,
/// the state leaves "installing".
fn watch_installer(state: AppState, mut child: std::process::Child) {
    tokio::spawn(async move {
        let waited = tokio::task::spawn_blocking(move || child.wait()).await;
        let updater = &state.0.update;
        let Ok(Ok(status)) = waited else {
            // Lost track of the installer: say so rather than "installing" forever.
            tracing::warn!("could not wait for the installer");
            updater.set_state(UpdateState::Failed, None, Some(INSTALL_FAILED));
            return;
        };
        if status.success() {
            tracing::info!("installer finished while Portal is still running");
            updater.set_state(UpdateState::Idle, None, None);
        } else {
            tracing::warn!("installer exited with {status}");
            updater.set_state(UpdateState::Failed, None, Some(INSTALL_FAILED));
        }
    });
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/update.rs"]
mod tests;
