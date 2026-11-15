//! Install a repository zip, then a video add-on published by that repository.
//! Download addresses stay in this module. Callers see ids and names only.

use crate::error::EngineError;
use flate2::read::GzDecoder;
use portal_api::{RepoAddon, RepoCard, RepoDetail, RepoList};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use zip::ZipArchive;

const DOWNLOAD_CAP: usize = 48 * 1024 * 1024;
const INDEX_CAP: usize = 24 * 1024 * 1024;
const XML_CAP: usize = 1024 * 1024;
const FILE_CAP: u64 = 32 * 1024 * 1024;
const UNCOMPRESSED_CAP: u64 = 96 * 1024 * 1024;
const FILE_COUNT: usize = 4000;
const PACKAGE_CAP: usize = 20_000;
const VIDEO_CAP: usize = 800;
const REPO_CAP: usize = 200;
const SCAN_CAP: usize = 5_000;
const CACHE_TTL: Duration = Duration::from_secs(600);
const DEP_DEPTH: usize = 12;
/// How long a server may take to connect and start its answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a download may go without a single byte before it is dropped.
/// A slow link that keeps sending is let finish.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// A download that trickles in forever still ends.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub struct InstallOutcome {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub wrote: bool,
}

#[derive(Debug)]
pub struct RemoveOutcome {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiskKind {
    Repository,
    Video,
    Module,
}

impl DiskKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Video => "video",
            Self::Module => "module",
        }
    }

    /// The add-on remove takes a video add-on or a module; repositories have
    /// their own.
    fn fits(self, expect: DiskKind) -> bool {
        self == expect || (expect == DiskKind::Video && self == DiskKind::Module)
    }
}

struct RepoFeed {
    info: String,
    compressed: bool,
    datadir: String,
}

struct DirCand {
    target: String,
    info: String,
    compressed: bool,
    datadir: String,
    zip: bool,
    /// Empty means no lower bound. Compared with the installed Kodi version.
    minversion: String,
    /// Empty means no upper bound.
    maxversion: String,
}

impl DirCand {
    fn blank() -> Self {
        Self {
            target: String::new(),
            info: String::new(),
            compressed: false,
            datadir: String::new(),
            zip: false,
            minversion: String::new(),
            maxversion: String::new(),
        }
    }
}

#[derive(Clone)]
struct Import {
    addon: String,
    optional: bool,
}

#[derive(Clone)]
struct Package {
    id: String,
    name: String,
    version: String,
    summary: String,
    zip: String,
    video: bool,
    /// A Python module other add-ons import.
    module: bool,
    imports: Vec<Import>,
}

struct CachedIndex {
    repo_id: String,
    at: Instant,
    name: String,
    packages: Vec<Package>,
}

struct Staged {
    id: String,
    name: String,
    kind: String,
    files: Vec<(String, Vec<u8>)>,
}

enum Accept {
    UserChoice,
    Listed(String),
}

enum Capture {
    None,
    Info,
    Datadir,
    Provides,
    Summary,
    Broken,
}

pub struct Addons {
    profile: PathBuf,
    programs: PathBuf,
    staging: PathBuf,
    client: Option<reqwest::Client>,
    cache: Mutex<Option<CachedIndex>>,
}

impl Addons {
    pub fn new(
        profile: impl Into<PathBuf>,
        programs: impl Into<PathBuf>,
        staging: impl Into<PathBuf>,
    ) -> Self {
        let programs = programs.into();
        let release = programs
            .parent()
            .and_then(|dir| file_version(&dir.join("kodi.exe")));
        let client = reqwest::Client::builder()
            .redirect(redirect_policy())
            // No limit on the whole download, which would cut off a large
            // zip on a slow link. The answer and each read are timed instead.
            .connect_timeout(ANSWER_TIMEOUT)
            // Many repositories serve the index and the zips only when the
            // client says it is Kodi. Anything else can be a redirect to a
            // web page, which is not an add-on index.
            .user_agent(repository_user_agent(release.as_deref()))
            .build()
            .ok();
        Self {
            profile: profile.into(),
            programs,
            staging: staging.into(),
            client,
            cache: Mutex::new(None),
        }
    }

    pub fn installed_repos(&self) -> Result<RepoList, EngineError> {
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        scan_repos(&self.profile.join("addons"), true, &mut seen, &mut items);
        scan_repos(&self.programs, false, &mut seen, &mut items);
        items.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then(left.id.cmp(&right.id))
        });
        items.truncate(REPO_CAP);
        Ok(RepoList { items })
    }

    /// The repository's video add-ons, then its Python modules: some video
    /// add-ons rely on a module from another repository, which the viewer
    /// installs here.
    pub async fn repo_videos(&self, repo_id: &str) -> Result<RepoDetail, EngineError> {
        let repo_id = require_addon_id(repo_id)?;
        let (name, packages) = self.index_for(&repo_id).await?;
        let listed = |want_video: bool| {
            let mut items: Vec<RepoAddon> = packages
                .iter()
                .filter(|package| {
                    if want_video {
                        package.video
                    } else {
                        package.module && !package.video
                    }
                })
                .map(|package| RepoAddon {
                    id: package.id.clone(),
                    name: package.name.clone(),
                    version: package.version.clone(),
                    summary: package.summary.clone(),
                    installed: self.on_disk(&package.id),
                    removable: self.video_removable(&package.id),
                    kind: if want_video { "video" } else { "module" }.to_string(),
                })
                .collect();
            items.sort_by(|left, right| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
                    .then(left.id.cmp(&right.id))
            });
            items.truncate(VIDEO_CAP);
            items
        };
        let mut items = listed(true);
        items.extend(listed(false));
        if items.is_empty() {
            return Err(EngineError::Message(
                "That repository has no video add-ons.".into(),
            ));
        }
        Ok(RepoDetail {
            id: repo_id,
            name,
            items,
        })
    }

    pub async fn install_zip(
        &self,
        url: &str,
        name: Option<&str>,
    ) -> Result<InstallOutcome, EngineError> {
        let url = self.resolve_install_link(url).await?;
        let bytes = self
            .download(&url, DOWNLOAD_CAP, "The download did not finish.")
            .await?;
        let mut outcome = self.stage_and_commit(bytes, Accept::UserChoice).await?;
        if let Some(label) = overlay_name(name) {
            outcome.name = label;
        }
        Ok(outcome)
    }

    /// Accept a direct https zip, or a repository site/root that lists a
    /// `repository.*.zip` (for example `https://repo.example.test`).
    async fn resolve_install_link(&self, raw: &str) -> Result<String, EngineError> {
        let url = raw.trim();
        if zip_address_ok(url) {
            return require_zip_address(url);
        }
        if !https_resource(url) {
            return Err(EngineError::BadRequest(
                "Enter a repository link or the address of an add-on zip.".into(),
            ));
        }
        let (page, landed) = self
            .download_at(url, INDEX_CAP, "That repository did not answer.")
            .await?;
        let html = String::from_utf8_lossy(&page);
        // Relative links belong to the address after redirects, not the one typed.
        let base = if https_resource(&landed) {
            landed
        } else {
            url.to_string()
        };
        pick_repo_zip_from_html(&html, &base)
    }

    pub async fn install_from_repo(
        &self,
        repo_id: &str,
        addon_id: &str,
    ) -> Result<InstallOutcome, EngineError> {
        let repo_id = require_addon_id(repo_id)?;
        let addon_id = require_addon_id(addon_id)?;
        let (_name, mut packages) = self.index_for(&repo_id).await?;
        let Some(target) = packages
            .iter()
            .find(|package| package.id == addon_id && (package.video || package.module))
        else {
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ));
        };
        let display = target.name.clone();
        let kind = if target.video { "video" } else { "module" };
        if self.on_disk(&addon_id) {
            return Ok(InstallOutcome {
                id: addon_id,
                name: display,
                kind: kind.to_string(),
                wrote: false,
            });
        }
        // Shared modules are often published by the
        // official repository, not by the video add-on's own repository.
        if plan_deps(&packages, &addon_id, &|id| self.on_disk(id)).is_err() {
            merge_packages(&mut packages, self.dependency_packages(&repo_id).await?);
        }
        let order = plan_deps(&packages, &addon_id, &|id| self.on_disk(id))?;
        let mut wrote = false;
        let mut last = InstallOutcome {
            id: addon_id.clone(),
            name: display,
            kind: kind.to_string(),
            wrote: false,
        };
        for id in order {
            let Some(package) = packages.iter().find(|package| package.id == id) else {
                self.drop_cache();
                return Err(EngineError::Message(
                    "A required add-on is not in this repository.".into(),
                ));
            };
            if self.on_disk(&package.id) {
                continue;
            }
            match self.install_package(package).await {
                Ok(outcome) => {
                    wrote |= outcome.wrote;
                    last = outcome;
                }
                Err(err) => {
                    self.drop_cache();
                    return Err(err);
                }
            }
        }
        last.wrote = wrote;
        last.kind = kind.to_string();
        Ok(last)
    }

    async fn index_for(&self, repo_id: &str) -> Result<(String, Vec<Package>), EngineError> {
        if let Some(hit) = self.cached(repo_id) {
            return Ok(hit);
        }
        let (name, xml) = read_repo_xml(&self.profile, &self.programs, repo_id)?;
        // The official Kodi repository publishes zip files without marking
        // its directory `zip="true"`. With no marked directory at all, the
        // unmarked ones are read the same way.
        let marked = read_feeds(&xml, self.kodi_release().as_deref(), true).is_ok();
        let packages = self.packages_in(&xml, marked).await?;
        self.store_cache(repo_id, &name, &packages);
        Ok((name, packages))
    }

    /// Packages from every other installed repository. A directory that does
    /// not mark itself `zip="true"` is still included: the official Kodi
    /// repository publishes zip files without that flag.
    async fn dependency_packages(&self, skip: &str) -> Result<Vec<Package>, EngineError> {
        let repos = self.installed_repos()?;
        let mut all = Vec::new();
        for card in repos.items {
            if card.id == skip {
                continue;
            }
            let Ok((_, xml)) = read_repo_xml(&self.profile, &self.programs, &card.id) else {
                continue;
            };
            match self.packages_in(&xml, false).await {
                Ok(packages) => merge_packages(&mut all, packages),
                Err(err) => {
                    tracing::warn!(
                        repo = %card.id,
                        error = %err.public_message(),
                        "dependency repository skipped"
                    );
                }
            }
        }
        Ok(all)
    }

    async fn packages_in(&self, xml: &str, require_zip: bool) -> Result<Vec<Package>, EngineError> {
        let feeds = read_feeds(xml, self.kodi_release().as_deref(), require_zip)?;
        let mut parts = Vec::with_capacity(feeds.len());
        for feed in &feeds {
            let part = match self.fetch_index(feed).await {
                Ok(body) => parse_index(&body, &feed.datadir),
                Err(err) => Err(err),
            };
            if let Err(err) = &part {
                tracing::warn!(
                    host = %download_host(&feed.info),
                    error = %err.public_message(),
                    "repository index skipped"
                );
            }
            parts.push(part);
        }
        merge_feed_results(parts)
    }

    /// `21.3` from `kodi.exe` beside the programs add-on folder. None when
    /// that file is not there, so a test profile does not filter directories.
    fn kodi_release(&self) -> Option<String> {
        let exe = self.programs.parent()?.join("kodi.exe");
        file_version(&exe)
    }

    async fn fetch_index(&self, feed: &RepoFeed) -> Result<String, EngineError> {
        let first = self
            .download(&feed.info, INDEX_CAP, "That repository did not answer.")
            .await?;
        let decoded = inflate_if_gzip(&first)?;
        if looks_like_xml(&decoded) {
            return Ok(utf8_xml(&decoded));
        }
        if feed.compressed && !feed.info.to_ascii_lowercase().ends_with(".gz") {
            let extra = format!("{}.gz", feed.info);
            if zip_host_ok(&extra) || https_resource(&extra) {
                if let Ok(bytes) = self
                    .download(&extra, INDEX_CAP, "That repository did not answer.")
                    .await
                {
                    let decoded = inflate_if_gzip(&bytes)?;
                    if looks_like_xml(&decoded) {
                        return Ok(utf8_xml(&decoded));
                    }
                }
            }
        }
        Err(EngineError::Message(
            "That repository could not be read.".into(),
        ))
    }

    async fn install_package(&self, package: &Package) -> Result<InstallOutcome, EngineError> {
        let bytes = self
            .download(&package.zip, DOWNLOAD_CAP, "The download did not finish.")
            .await?;
        self.stage_and_commit(bytes, Accept::Listed(package.id.clone()))
            .await
    }

    /// Unzipping (up to `UNCOMPRESSED_CAP`) and writing the files run on a
    /// blocking thread. `commit` only reads the three folders, so a copy
    /// without the client or the index cache is enough there.
    async fn stage_and_commit(
        &self,
        bytes: Vec<u8>,
        accept: Accept,
    ) -> Result<InstallOutcome, EngineError> {
        let disk = Addons {
            profile: self.profile.clone(),
            programs: self.programs.clone(),
            staging: self.staging.clone(),
            client: None,
            cache: Mutex::new(None),
        };
        tokio::task::spawn_blocking(move || disk.commit(stage_zip(&bytes, accept)?))
            .await
            .map_err(|_| EngineError::Message("Could not install this add-on.".into()))?
    }

    fn commit(&self, staged: Staged) -> Result<InstallOutcome, EngineError> {
        if self.on_disk(&staged.id) {
            return Ok(InstallOutcome {
                id: staged.id,
                name: staged.name,
                kind: staged.kind,
                wrote: false,
            });
        }
        self.write_staged(&staged)?;
        Ok(InstallOutcome {
            id: staged.id,
            name: staged.name,
            kind: staged.kind,
            wrote: true,
        })
    }

    fn write_staged(&self, staged: &Staged) -> Result<(), EngineError> {
        if reserved_addon_id(&staged.id) {
            return Err(EngineError::Message("That file is not an add-on.".into()));
        }
        // One install at a time: two installs of the same add-on (or of a
        // shared module) must not write into each other's folder.
        static INSTALLING: Mutex<()> = Mutex::new(());
        let _one = INSTALLING.lock().unwrap_or_else(|err| err.into_inner());
        let stage = self
            .staging
            .join(format!("{}.{}", staged.id, std::process::id()));
        let _ = fs::remove_dir_all(&stage);
        if fs::create_dir_all(&stage).is_err() {
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ));
        }
        let fail = |stage: &Path| {
            let _ = fs::remove_dir_all(stage);
            EngineError::Message("Could not install this add-on.".into())
        };
        for (rel, bytes) in &staged.files {
            let Some(path) = under(&stage, rel) else {
                return Err(fail(&stage));
            };
            if let Some(parent) = path.parent() {
                if fs::create_dir_all(parent).is_err() {
                    return Err(fail(&stage));
                }
            }
            if fs::write(&path, bytes).is_err() {
                return Err(fail(&stage));
            }
        }
        if !stage.join("addon.xml").is_file() {
            let _ = fs::remove_dir_all(&stage);
            return Err(EngineError::Message("That file is not an add-on.".into()));
        }
        let dest_root = self.profile.join("addons");
        if fs::create_dir_all(&dest_root).is_err() {
            return Err(fail(&stage));
        }
        let dest = dest_root.join(&staged.id);
        if dest.join("addon.xml").is_file() || self.on_disk(&staged.id) {
            let _ = fs::remove_dir_all(&stage);
            return Ok(());
        }
        // A folder without addon.xml is not this add-on; it is never merged into.
        if dest.exists() {
            let _ = fs::remove_dir_all(&stage);
            return Err(EngineError::Message(
                "A folder by that add-on's name is already there.".into(),
            ));
        }
        if fs::rename(&stage, &dest).is_ok() {
            return Ok(());
        }
        if copy_tree(&stage, &dest).is_err() {
            let _ = fs::remove_dir_all(&stage);
            let _ = fs::remove_dir_all(&dest);
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ));
        }
        let _ = fs::remove_dir_all(&stage);
        Ok(())
    }

    fn on_disk(&self, id: &str) -> bool {
        self.profile
            .join("addons")
            .join(id)
            .join("addon.xml")
            .is_file()
            || self.programs.join(id).join("addon.xml").is_file()
    }

    /// True when this id is a video add-on or a module written under the Kodi
    /// profile.
    pub fn video_removable(&self, id: &str) -> bool {
        let Ok(id) = require_addon_id(id) else {
            return false;
        };
        if reserved_addon_dir(&id) {
            return false;
        }
        let dir = self.profile.join("addons").join(&id);
        matches!(read_profile_kind(&dir, &id), Ok((_, DiskKind::Video | DiskKind::Module)))
    }

    pub fn remove_repository(&self, id: &str) -> Result<RemoveOutcome, EngineError> {
        self.remove(id, DiskKind::Repository)
    }

    pub fn remove_video(&self, id: &str) -> Result<RemoveOutcome, EngineError> {
        self.remove(id, DiskKind::Video)
    }

    /// Delete one profile add-on. Program Files copies, other add-ons, and the
    /// package cache folder itself are left in place. A half-finished delete
    /// can be asked again: the staging marker remembers the kind already checked.
    fn remove(&self, raw: &str, expect: DiskKind) -> Result<RemoveOutcome, EngineError> {
        let id = require_addon_id(raw)?;
        if reserved_addon_dir(&id) {
            return Err(EngineError::BadRequest("Unknown add-on.".into()));
        }
        let addons_root = self.profile.join("addons");
        let data_root = self.profile.join("userdata").join("addon_data");
        let addon_dir = addons_root.join(&id);
        let data_dir = data_root.join(&id);
        let packages = addons_root.join("packages");
        if !direct_child(&addons_root, &addon_dir, &id) || !direct_child(&data_root, &data_dir, &id)
        {
            return Err(EngineError::BadRequest("Unknown add-on.".into()));
        }
        let marker = self.staging.join(format!("remove-{id}"));
        let (name, kind) = if addon_dir.join("addon.xml").is_file() {
            let (name, kind) = read_profile_kind(&addon_dir, &id)?;
            if !kind.fits(expect) {
                return Err(wrong_kind(kind, expect));
            }
            (name, kind)
        } else if let Some(kind) = [expect, DiskKind::Module]
            .into_iter()
            .find(|kind| kind.fits(expect) && marker_is(&marker, *kind))
        {
            (id.clone(), kind)
        } else if self.programs.join(&id).join("addon.xml").is_file() {
            return Err(EngineError::Message(
                "That add-on comes with Kodi and stays installed.".into(),
            ));
        } else {
            return Err(EngineError::Message("That add-on is not installed.".into()));
        };
        if kind == DiskKind::Module {
            if let Some(user) = self.module_user(&id) {
                return Err(EngineError::Message(format!(
                    "{user} uses this module. Remove that add-on first."
                )));
            }
        }
        guard_tree(&addon_dir)?;
        guard_tree(&data_dir)?;
        guard_packages(&packages, &id)?;
        if fs::create_dir_all(&self.staging).is_err()
            || fs::write(&marker, kind.as_str()).is_err()
        {
            return Err(EngineError::Message(
                "That add-on could not be removed.".into(),
            ));
        }
        delete_tree(&addon_dir)?;
        delete_tree(&data_dir)?;
        delete_packages(&packages, &id)?;
        let _ = fs::remove_file(&marker);
        self.drop_cache();
        Ok(RemoveOutcome {
            id,
            name,
            kind: kind.as_str().to_string(),
        })
    }

    /// The name of an installed add-on that needs module `id`, if any.
    fn module_user(&self, id: &str) -> Option<String> {
        let entries = fs::read_dir(self.profile.join("addons")).ok()?;
        for entry in entries.flatten().take(SCAN_CAP) {
            let xml_path = entry.path().join("addon.xml");
            let Some(xml) = read_capped(&xml_path, XML_CAP) else {
                continue;
            };
            if requires_addon(&xml, id) {
                let name = addon_name(&xml)
                    .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
                return Some(collapse(&name, 200));
            }
        }
        None
    }

    fn cached(&self, repo_id: &str) -> Option<(String, Vec<Package>)> {
        let guard = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        let slot = guard.as_ref()?;
        if slot.repo_id != repo_id || slot.at.elapsed() > CACHE_TTL {
            return None;
        }
        Some((slot.name.clone(), slot.packages.clone()))
    }

    fn store_cache(&self, repo_id: &str, name: &str, packages: &[Package]) {
        let mut guard = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        *guard = Some(CachedIndex {
            repo_id: repo_id.to_string(),
            at: Instant::now(),
            name: name.to_string(),
            packages: packages.to_vec(),
        });
    }

    fn drop_cache(&self) {
        *self.cache.lock().unwrap_or_else(|err| err.into_inner()) = None;
    }

    async fn download_at(
        &self,
        url: &str,
        cap: usize,
        fail: &'static str,
    ) -> Result<(Vec<u8>, String), EngineError> {
        if !https_resource(url) && !zip_address_ok(url) {
            return Err(EngineError::Unavailable(fail));
        }
        let Some(client) = self.client.as_ref() else {
            return Err(EngineError::Unavailable(fail));
        };
        let mut response = match tokio::time::timeout(ANSWER_TIMEOUT, client.get(url).send()).await
        {
            Ok(Ok(response)) => response,
            _ => return Err(EngineError::Unavailable(fail)),
        };
        let landed = response.url().as_str().to_string();
        if !response.status().is_success() {
            let status = response.status().as_u16();
            tracing::warn!(status, host = %download_host(&landed), "add-on download failed");
            if status == 404 {
                return Err(EngineError::Message("That file was not found.".into()));
            }
            return Err(EngineError::Unavailable(fail));
        }
        let bytes = gather(&mut response, cap, STALL_TIMEOUT, DOWNLOAD_TIMEOUT)
            .await
            .ok_or(EngineError::Unavailable(fail))?;
        Ok((bytes, landed))
    }

    async fn download(
        &self,
        url: &str,
        cap: usize,
        fail: &'static str,
    ) -> Result<Vec<u8>, EngineError> {
        Ok(self.download_at(url, cap, fail).await?.0)
    }
}

/// A body read a piece at a time. A trait so the timing can be tested
/// without a server.
trait Chunks {
    /// The next piece, None at the end, or Err when the read failed.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ()>;
}

impl Chunks for reqwest::Response {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ()> {
        match self.chunk().await {
            Ok(chunk) => Ok(chunk.map(Vec::from)),
            Err(_) => Err(()),
        }
    }
}

/// Read a whole body of at most `cap` bytes. It fails once nothing arrives
/// for `stall`, or once `total` has passed, but a body that keeps coming
/// may take as long as it needs within that.
async fn gather(
    body: &mut impl Chunks,
    cap: usize,
    stall: Duration,
    total: Duration,
) -> Option<Vec<u8>> {
    let end = tokio::time::Instant::now() + total;
    let mut out = Vec::new();
    loop {
        let wait = (tokio::time::Instant::now() + stall).min(end);
        let chunk = match tokio::time::timeout_at(wait, body.next_chunk()).await {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => return Some(out),
            _ => return None,
        };
        if out.len().saturating_add(chunk.len()) > cap {
            return None;
        }
        out.extend_from_slice(&chunk);
    }
}

fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 3 {
            return attempt.error(std::io::Error::other("redirects"));
        }
        let url = attempt.url();
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return attempt.error(std::io::Error::other("redirects"));
        }
        let text = url.as_str();
        if text.len() > 2048
            || text.contains('@')
            || text.contains('\\')
            || text.contains('#')
            || text.matches("://").count() != 1
            || !text.bytes().all(|byte| (0x21..=0x7E).contains(&byte))
        {
            return attempt.error(std::io::Error::other("redirects"));
        }
        attempt.follow()
    })
}

fn require_addon_id(value: &str) -> Result<String, EngineError> {
    let id = value.trim();
    let ok = (1..=80).contains(&id.len())
        && !id.contains("..")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !ok {
        return Err(EngineError::BadRequest("Unknown add-on.".into()));
    }
    Ok(id.to_string())
}

fn valid_version(version: &str) -> bool {
    (1..=64).contains(&version.len())
        && version.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+' | b'~')
        })
}

fn https_resource(url: &str) -> bool {
    if url.len() > 2048 || !url.starts_with("https://") || url.matches("://").count() != 1 {
        return false;
    }
    if url.contains('@') || url.contains('\\') || url.contains('#') {
        return false;
    }
    url.bytes().all(|byte| (0x21..=0x7E).contains(&byte))
}

fn zip_address_ok(url: &str) -> bool {
    if !https_resource(url) {
        return false;
    }
    let path = url.split('?').next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".zip")
}

fn zip_host_ok(url: &str) -> bool {
    https_resource(url)
}

fn require_zip_address(raw: &str) -> Result<String, EngineError> {
    let url = raw.trim();
    if url.is_empty() || !zip_address_ok(url) {
        return Err(EngineError::BadRequest(
            "Enter a repository link or the address of an add-on zip.".into(),
        ));
    }
    Ok(url.to_string())
}

fn html_hrefs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Lowercased once for the whole page. ASCII lowercasing keeps every byte
    // offset, so a match here is the same offset in `html`.
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i + 6 < bytes.len() {
        let Some(rel) = lower[i..].find("href=").map(|at| i + at + 5) else {
            break;
        };
        i = rel;
        if i >= html.len() {
            break;
        }
        let quote = bytes[i];
        let (start, end) = if quote == b'"' || quote == b'\'' {
            let start = i + 1;
            let Some(close) = html[start..].find(quote as char) else {
                break;
            };
            (start, start + close)
        } else {
            let start = i;
            let end = html[start..]
                .find(|c: char| c.is_whitespace() || c == '>')
                .map(|n| start + n)
                .unwrap_or(html.len());
            (start, end)
        };
        // An unquoted value can end on a multi-byte space such as U+00A0, so
        // the next search starts on that character, not one byte into it.
        i = if quote == b'"' || quote == b'\'' {
            end + 1
        } else {
            end
        };
        let href = html[start..end]
            .replace("&amp;", "&")
            .replace("&#39;", "'")
            .replace("&quot;", "\"")
            .trim()
            .to_string();
        if !href.is_empty() {
            out.push(href);
        }
    }
    out
}

fn join_https(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty()
        || href.starts_with('#')
        || href.to_ascii_lowercase().starts_with("javascript:")
        || href.to_ascii_lowercase().starts_with("data:")
    {
        return None;
    }
    if href.starts_with("https://") {
        return if https_resource(href) {
            Some(href.to_string())
        } else {
            None
        };
    }
    if href.starts_with("http://") || href.starts_with("//") {
        return None;
    }
    if !https_resource(base) {
        return None;
    }
    if href.starts_with('/') {
        let host = base.strip_prefix("https://")?.split('/').next()?;
        let out = format!("https://{host}{href}");
        return if https_resource(&out) {
            Some(out)
        } else {
            None
        };
    }
    let prefix = if base.ends_with('/') {
        base.to_string()
    } else {
        match base.rfind('/') {
            // `idx` is the slash. Including it and then adding another made
            // `https://host/index.html` resolve to `https://host//file`.
            Some(idx) if idx > "https://".len() => format!("{}/", &base[..idx]),
            _ => format!("{base}/"),
        }
    };
    let out = format!("{prefix}{href}");
    if https_resource(&out) {
        Some(out)
    } else {
        None
    }
}

fn zip_file_name(url: &str) -> String {
    url.split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// A repository page is a folder. `https://host/packages` (no slash) still
/// holds `repository.name.zip` inside that folder. A last segment with a dot
/// stays a file, so a link next to `index.html` is unchanged.
fn folder_base(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    if !https_resource(url) || path.ends_with('/') {
        return url.to_string();
    }
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return url.to_string();
    }
    // The query is not part of the folder. Leaving it on the base made the
    // joiner see the new slash and emit a double slash.
    match url.split_once('?') {
        Some((path, _)) => format!("{path}/"),
        None => format!("{url}/"),
    }
}

fn download_host(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
}

fn pick_repo_zip_from_html(html: &str, base: &str) -> Result<String, EngineError> {
    let base = folder_base(base);
    let mut zips = Vec::new();
    for href in html_hrefs(html) {
        let Some(url) = join_https(&base, &href) else {
            continue;
        };
        if zip_address_ok(&url) {
            zips.push(url);
        }
    }
    if zips.is_empty() {
        return Err(EngineError::BadRequest(
            "No repository zip was found at that link.".into(),
        ));
    }
    if let Some(url) = zips.iter().find(|url| {
        let name = zip_file_name(url);
        name.starts_with("repository.") && name.ends_with(".zip")
    }) {
        return Ok(url.clone());
    }
    if let Some(url) = zips
        .iter()
        .find(|url| zip_file_name(url).contains("repository"))
    {
        return Ok(url.clone());
    }
    Err(EngineError::BadRequest(
        "No repository zip was found at that link.".into(),
    ))
}

fn overlay_name(raw: Option<&str>) -> Option<String> {
    let name = raw?.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_string())
}

fn package_url(datadir: &str, id: &str, version: &str) -> Option<String> {
    if require_addon_id(id).is_err() || !valid_version(version) || !https_resource(datadir) {
        return None;
    }
    let base = if datadir.ends_with('/') {
        datadir.to_string()
    } else {
        format!("{datadir}/")
    };
    let url = format!("{base}{id}/{id}-{version}.zip");
    if zip_address_ok(&url) {
        Some(url)
    } else {
        None
    }
}

fn slash_owned(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

fn rank_dir(dir: &DirCand) -> u8 {
    let target = dir.target.as_str();
    if target.is_empty() || target == "all" || target == "universal" || target == "any" {
        0
    } else if target == "windows" || target == "win32" || target.starts_with("windows") {
        1
    } else {
        2
    }
}

/// Directories Kodi itself would fetch. `kodi` is the installed version
/// (`21.3`). Directories outside that range are dropped. Several directories
/// can match one version, and the same add-on keeps the higher version.
/// `require_zip` is set for the repository the user opened. Dependency
/// lookups leave it off so the official repository, which omits `zip="true"`,
/// is still searched.
fn choose_feeds(
    dirs: &[DirCand],
    kodi: Option<&str>,
    require_zip: bool,
) -> Result<Vec<RepoFeed>, EngineError> {
    if dirs.is_empty() {
        return Err(EngineError::BadRequest(
            "That address is not a repository zip.".into(),
        ));
    }
    let mut https: Vec<&DirCand> = dirs
        .iter()
        .filter(|dir| https_resource(&dir.info) && https_resource(&dir.datadir))
        .filter(|dir| !require_zip || dir.zip)
        .collect();
    if https.is_empty() {
        return Err(EngineError::BadRequest(if require_zip {
            "This repository does not publish zip add-ons.".into()
        } else {
            "That address is not a repository zip.".into()
        }));
    }
    if let Some(kodi) = kodi {
        let fitted: Vec<&DirCand> = https
            .iter()
            .copied()
            .filter(|dir| version_fits(kodi, &dir.minversion, &dir.maxversion))
            .collect();
        if fitted.is_empty() {
            return Err(EngineError::Message(
                "That repository could not be read.".into(),
            ));
        }
        https = fitted;
    }
    https.sort_by_key(|dir| rank_dir(dir));
    let best = rank_dir(https[0]);
    https.retain(|dir| rank_dir(dir) == best);
    Ok(https
        .into_iter()
        .map(|dir| RepoFeed {
            info: dir.info.clone(),
            compressed: dir.compressed,
            datadir: slash_owned(&dir.datadir),
        })
        .collect())
}

fn read_feed(xml: &str) -> Result<RepoFeed, EngineError> {
    let mut feeds = read_feeds(xml, None, true)?;
    Ok(feeds.remove(0))
}

fn read_feeds(
    xml: &str,
    kodi: Option<&str>,
    require_zip: bool,
) -> Result<Vec<RepoFeed>, EngineError> {
    let meta = parse_addon_xml(xml)?;
    if !meta
        .points
        .iter()
        .any(|point| point == "xbmc.addon.repository")
    {
        return Err(EngineError::Message(
            "That repository could not be read.".into(),
        ));
    }
    choose_feeds(&meta.dirs, kodi, require_zip)
}

fn version_parts(raw: &str) -> Vec<u32> {
    raw.split(|ch: char| ch != '.' && !ch.is_ascii_digit())
        .next()
        .unwrap_or("")
        .split('.')
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn version_cmp(left: &str, right: &str) -> Ordering {
    let left = version_parts(left);
    let right = version_parts(right);
    let width = left.len().max(right.len());
    for index in 0..width {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        if a != b {
            return a.cmp(&b);
        }
    }
    Ordering::Equal
}

fn version_fits(kodi: &str, minversion: &str, maxversion: &str) -> bool {
    let min_ok = minversion.trim().is_empty() || version_cmp(kodi, minversion) != Ordering::Less;
    let max_ok = maxversion.trim().is_empty() || version_cmp(kodi, maxversion) != Ordering::Greater;
    min_ok && max_ok
}

/// A directory that does not answer, or whose body is not an add-on index,
/// is left out. The directories that did load are still returned. The last
/// error is kept for a repository where every directory failed.
fn merge_feed_results(
    parts: Vec<Result<Vec<Package>, EngineError>>,
) -> Result<Vec<Package>, EngineError> {
    let mut packages = Vec::new();
    let mut loaded = false;
    let mut last = None;
    for part in parts {
        match part {
            Ok(found) => {
                loaded = true;
                merge_packages(&mut packages, found);
            }
            Err(err) => last = Some(err),
        }
    }
    if !loaded {
        return Err(
            last.unwrap_or_else(|| EngineError::Message("That repository did not answer.".into()))
        );
    }
    Ok(packages)
}

/// Portal, and the Kodi it fetches the add-ons for: a repository may serve
/// a different list to each Kodi version.
fn repository_user_agent(release: Option<&str>) -> String {
    let portal = concat!("Portal/", env!("CARGO_PKG_VERSION"));
    match release.map(str::trim).filter(|version| !version.is_empty()) {
        Some(version) => format!("{portal} Kodi/{version}"),
        None => format!("{portal} Kodi"),
    }
}

fn merge_packages(into: &mut Vec<Package>, extra: Vec<Package>) {
    for package in extra {
        if let Some(pos) = into.iter().position(|have| have.id == package.id) {
            if version_cmp(&package.version, &into[pos].version) == Ordering::Greater {
                into[pos] = package;
            }
        } else {
            into.push(package);
        }
    }
}

fn file_version(path: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<Vec<(PathBuf, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let guard = cache.lock().unwrap_or_else(|err| err.into_inner());
    if let Some((_, version)) = guard.iter().find(|(have, _)| have == path) {
        return Some(version.clone());
    }
    drop(guard);
    let bytes = fs::read(path).ok()?;
    let version = version_marked(&bytes)?;
    let mut guard = cache.lock().unwrap_or_else(|err| err.into_inner());
    guard.push((path.to_path_buf(), version.clone()));
    Some(version)
}

/// The `FileVersion` string in a Windows version resource.
fn version_marked(bytes: &[u8]) -> Option<String> {
    let mut key = Vec::new();
    for unit in "FileVersion".encode_utf16() {
        key.extend_from_slice(&unit.to_le_bytes());
    }
    key.extend_from_slice(&[0, 0]);
    let pos = bytes
        .windows(key.len())
        .position(|window| window == key.as_slice())?;
    let mut index = pos + key.len();
    if index + 1 < bytes.len() && bytes[index] == 0 && bytes[index + 1] == 0 {
        index += 2;
    }
    let mut out = String::new();
    while index + 1 < bytes.len() {
        let unit = u16::from_le_bytes([bytes[index], bytes[index + 1]]);
        index += 2;
        if unit == 0 {
            break;
        }
        let ch = char::from_u32(u32::from(unit))?;
        if out.len() > 32 {
            return None;
        }
        out.push(ch);
    }
    let out = out.trim();
    if version_parts(out).is_empty() {
        None
    } else {
        Some(out.to_string())
    }
}

struct AddonMeta {
    id: String,
    name: String,
    points: Vec<String>,
    provides: String,
    broken: String,
    dirs: Vec<DirCand>,
}

/// The `name` an add-on's addon.xml gives it, when the file parses.
pub(crate) fn addon_name(xml: &str) -> Option<String> {
    parse_addon_xml(xml)
        .ok()
        .map(|meta| meta.name)
        .filter(|name| !name.trim().is_empty())
}

fn parse_addon_xml(xml: &str) -> Result<AddonMeta, EngineError> {
    let xml = xml.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut meta = AddonMeta {
        id: String::new(),
        name: String::new(),
        points: Vec::new(),
        provides: String::new(),
        broken: String::new(),
        dirs: Vec::new(),
    };
    let mut capture = Capture::None;
    let mut repo_depth: Option<usize> = None;
    let mut dir_at: Option<usize> = None;
    let mut current = DirCand::blank();
    let mut loose = DirCand::blank();
    let mut saw_dir = false;
    loop {
        if stack.len() > 64 {
            return Err(EngineError::Message(
                "That repository could not be read.".into(),
            ));
        }
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "addon" && meta.id.is_empty() {
                    meta.id = attr_value(&event, "id");
                    meta.name = attr_value(&event, "name");
                } else if name == "extension" {
                    let point = attr_value(&event, "point");
                    if !point.is_empty() {
                        meta.points.push(point.clone());
                    }
                    if point == "xbmc.addon.repository" {
                        repo_depth = Some(stack.len());
                        saw_dir = false;
                        loose = DirCand::blank();
                    }
                } else if name == "dir" && repo_depth.is_some() && dir_at.is_none() {
                    saw_dir = true;
                    dir_at = Some(stack.len());
                    current = DirCand {
                        target: attr_value(&event, "target").to_ascii_lowercase(),
                        minversion: attr_value(&event, "minversion"),
                        maxversion: attr_value(&event, "maxversion"),
                        ..DirCand::blank()
                    };
                } else if name == "info" && repo_depth.is_some() {
                    let compressed = attr_value(&event, "compressed").eq_ignore_ascii_case("true");
                    if dir_at.is_some() {
                        current.compressed = compressed;
                    } else {
                        loose.compressed = compressed;
                    }
                    capture = Capture::Info;
                } else if name == "datadir" && repo_depth.is_some() {
                    let zipped = attr_value(&event, "zip").eq_ignore_ascii_case("true");
                    if dir_at.is_some() {
                        current.zip = zipped;
                    } else {
                        loose.zip = zipped;
                    }
                    capture = Capture::Datadir;
                } else if name == "provides" {
                    capture = Capture::Provides;
                } else if name == "broken" {
                    capture = Capture::Broken;
                }
                if name == "summary" {
                    let lang = attr_value(&event, "lang").to_ascii_lowercase();
                    capture = if want_summary(&lang) {
                        Capture::Summary
                    } else {
                        Capture::None
                    };
                }
                stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "extension" {
                    let point = attr_value(&event, "point");
                    if !point.is_empty() {
                        meta.points.push(point);
                    }
                } else if name == "import" {
                    // Imports are read from the repository index, not from a single addon.xml here.
                }
            }
            Ok(Event::Text(event)) => {
                let Ok(text) = event.unescape() else {
                    buf.clear();
                    continue;
                };
                match capture {
                    Capture::Info => {
                        if dir_at.is_some() {
                            current.info.push_str(text.trim());
                        } else if repo_depth.is_some() {
                            loose.info.push_str(text.trim());
                        }
                    }
                    Capture::Datadir => {
                        if dir_at.is_some() {
                            current.datadir.push_str(text.trim());
                        } else if repo_depth.is_some() {
                            loose.datadir.push_str(text.trim());
                        }
                    }
                    Capture::Provides => {
                        if meta.provides.len() < 80 {
                            meta.provides.push_str(&text);
                        }
                    }
                    Capture::Summary => {
                        // Stored in name overflow? Use a side channel on meta via broken no.
                        // Summary for a user zip is not required. Index parser collects it.
                    }
                    Capture::Broken => {
                        if meta.broken.len() < 200 {
                            meta.broken.push_str(&text);
                        }
                    }
                    Capture::None => {}
                }
            }
            Ok(Event::End(_)) => {
                let ended = stack.pop();
                if ended.as_deref() == Some("info")
                    || ended.as_deref() == Some("datadir")
                    || ended.as_deref() == Some("provides")
                    || ended.as_deref() == Some("summary")
                    || ended.as_deref() == Some("broken")
                {
                    capture = Capture::None;
                }
                if ended.as_deref() == Some("dir") && dir_at == Some(stack.len()) {
                    meta.dirs.push(take_dir(&mut current));
                    dir_at = None;
                }
                if ended.as_deref() == Some("extension") && repo_depth == Some(stack.len()) {
                    if !saw_dir && (!loose.info.is_empty() || !loose.datadir.is_empty()) {
                        meta.dirs.push(take_dir(&mut loose));
                    }
                    repo_depth = None;
                    saw_dir = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => {
                return Err(EngineError::Message(
                    "That repository could not be read.".into(),
                ))
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(meta)
}

/// True when this addon.xml imports `id` and the import is not optional.
fn requires_addon(xml: &str, id: &str) -> bool {
    let mut reader = Reader::from_str(xml.trim_start_matches('\u{feff}'));
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if local_name(event.name().as_ref()) == "import"
                    && attr_value(&event, "addon") == id
                    && !attr_value(&event, "optional").eq_ignore_ascii_case("true")
                {
                    return true;
                }
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
        buf.clear();
    }
}

fn want_summary(lang: &str) -> bool {
    lang.is_empty() || lang.starts_with("en")
}

fn take_dir(dir: &mut DirCand) -> DirCand {
    DirCand {
        target: std::mem::take(&mut dir.target),
        info: std::mem::take(&mut dir.info).trim().to_string(),
        compressed: dir.compressed,
        datadir: std::mem::take(&mut dir.datadir).trim().to_string(),
        zip: dir.zip,
        minversion: std::mem::take(&mut dir.minversion).trim().to_string(),
        maxversion: std::mem::take(&mut dir.maxversion).trim().to_string(),
    }
}

fn stage_zip(bytes: &[u8], accept: Accept) -> Result<Staged, EngineError> {
    if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
        return Err(EngineError::BadRequest("That file is not a zip.".into()));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| EngineError::BadRequest("That file is not a zip.".into()))?;
    if archive.len() > FILE_COUNT {
        return Err(EngineError::Message(
            "Could not install this add-on.".into(),
        ));
    }
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|_| EngineError::Message("Could not install this add-on.".into()))?;
        if file.is_dir() {
            continue;
        }
        if file.encrypted() || file.is_symlink() || is_link_mode(file.unix_mode()) {
            if file.encrypted() {
                return Err(EngineError::Message(
                    "Could not install this add-on.".into(),
                ));
            }
            continue;
        }
        let Some(enclosed) = file.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let Some(rel) = relative_zip_path(&enclosed) else {
            continue;
        };
        if file.size() > FILE_CAP {
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ));
        }
        if total.saturating_add(file.size()) > UNCOMPRESSED_CAP {
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ));
        }
        // The declared size can lie, so the read itself stops one byte past
        // the cap and the total counts what was actually read.
        let mut buf = Vec::new();
        (&mut file)
            .take(FILE_CAP + 1)
            .read_to_end(&mut buf)
            .map_err(|_| EngineError::Message("Could not install this add-on.".into()))?;
        total = total.saturating_add(buf.len() as u64);
        if buf.len() as u64 > FILE_CAP || total > UNCOMPRESSED_CAP {
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ));
        }
        entries.push((rel, buf));
    }
    let mut xml_folders: Vec<(String, String)> = Vec::new();
    for (rel, bytes) in &entries {
        if let Some(folder) = addon_xml_folder(rel) {
            let text = String::from_utf8_lossy(bytes);
            if text.len() > XML_CAP {
                return Err(EngineError::Message(
                    "Could not install this add-on.".into(),
                ));
            }
            xml_folders.push((folder, text.into_owned()));
        }
    }
    if xml_folders.is_empty() {
        return Err(EngineError::BadRequest(
            "That file is not an add-on.".into(),
        ));
    }
    if xml_folders.len() > 1 {
        return Err(EngineError::BadRequest(
            "That file contains more than one add-on.".into(),
        ));
    }
    let (folder, xml) = &xml_folders[0];
    let meta = parse_addon_xml(xml)?;
    let id = require_addon_id(&meta.id)?;
    // Zip root folder often matches the add-on id. Some repos use a
    // site-style folder while addon.xml id differs; install under meta.id.
    if !folder.is_empty()
        && (folder.contains('/')
            || folder.contains('\\')
            || folder.contains("..")
            || !folder
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    {
        return Err(EngineError::Message(
            "Could not install this add-on.".into(),
        ));
    }
    if !meta.broken.trim().is_empty() {
        return Err(EngineError::Message(
            "Could not install this add-on.".into(),
        ));
    }
    let repository = meta
        .points
        .iter()
        .any(|point| point == "xbmc.addon.repository");
    let video = is_video(&meta.points, &meta.provides);
    let kind = match &accept {
        Accept::UserChoice if repository => {
            let _feed = read_feed(xml)?;
            "repository"
        }
        Accept::UserChoice if video => "video",
        Accept::UserChoice if is_module(&meta.points) => "module",
        Accept::UserChoice => {
            return Err(EngineError::BadRequest(
                "Portal installs a repository, a video add-on, or a module video add-ons use.".into(),
            ))
        }
        Accept::Listed(expected) if expected == &id => {
            if repository {
                "repository"
            } else if video {
                "video"
            } else {
                "module"
            }
        }
        Accept::Listed(_) => {
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ))
        }
    };
    let mut files = Vec::new();
    for (rel, bytes) in entries {
        let Some(inside) = strip_folder(&rel, folder) else {
            continue;
        };
        files.push((inside, bytes));
    }
    if !files.iter().any(|(rel, _)| rel == "addon.xml") {
        return Err(EngineError::BadRequest(
            "That file is not an add-on.".into(),
        ));
    }
    let name = collapse(&meta.name, 200);
    let name = if name.is_empty() { id.clone() } else { name };
    Ok(Staged {
        id,
        name,
        kind: kind.to_string(),
        files,
    })
}

fn is_link_mode(mode: Option<u32>) -> bool {
    match mode {
        Some(mode) => mode & 0o170000 == 0o120000,
        None => false,
    }
}

fn relative_zip_path(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str()?;
                if text.is_empty()
                    || text == ".."
                    || text.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
                    || text.ends_with(['.', ' '])
                    || reserved_windows_name(text)
                    || text.len() > 180
                {
                    return None;
                }
                parts.push(text.to_string());
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    if parts.is_empty() || parts.len() > 16 {
        return None;
    }
    Some(parts.join("/"))
}

/// A name Windows keeps for a device (`CON`, `NUL`, `COM1`...), with or
/// without an extension. A file by that name cannot be written or removed
/// normally.
fn reserved_windows_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
}

/// Folders Kodi keeps for itself under `addons`.
fn reserved_addon_id(id: &str) -> bool {
    matches!(id.to_ascii_lowercase().as_str(), "packages" | "temp")
}

fn addon_xml_folder(rel: &str) -> Option<String> {
    let parts: Vec<&str> = rel.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() == 1 && parts[0] == "addon.xml" {
        return Some(String::new());
    }
    if parts.len() == 2 && parts[1] == "addon.xml" {
        return Some(parts[0].to_string());
    }
    None
}

fn strip_folder(rel: &str, folder: &str) -> Option<String> {
    if folder.is_empty() {
        return Some(rel.to_string());
    }
    let prefix = format!("{folder}/");
    rel.strip_prefix(&prefix).map(str::to_string)
}

fn is_video(points: &[String], provides: &str) -> bool {
    let legacy = points.iter().any(|point| point == "xbmc.addon.video");
    let plugin = points
        .iter()
        .any(|point| point == "xbmc.python.pluginsource" || point == "xbmc.addon.video");
    if legacy && provides.trim().is_empty() {
        return true;
    }
    plugin && provides_video(provides)
}

fn is_module(points: &[String]) -> bool {
    points.iter().any(|point| point == "xbmc.python.module")
}

fn keep_in_index(points: &[String], provides: &str) -> bool {
    // A module can also run as a program of its own; it is still something
    // video add-ons import.
    if is_video(points, provides) || is_module(points) {
        return true;
    }
    let playback = points.iter().any(|point| {
        point == "xbmc.python.pluginsource"
            || point == "xbmc.addon.video"
            || point == "xbmc.addon.audio"
            || point == "xbmc.addon.image"
            || point == "xbmc.addon.executable"
    });
    !playback
}

fn provides_video(text: &str) -> bool {
    text.split(|ch: char| ch.is_whitespace() || ch == ',')
        .any(|part| part.eq_ignore_ascii_case("video"))
}

fn parse_index(xml: &str, datadir: &str) -> Result<Vec<Package>, EngineError> {
    let xml = xml.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut packages = Vec::new();
    let mut build: Option<IndexBuild> = None;
    loop {
        if stack.len() > 64 {
            return Err(EngineError::Message(
                "That repository could not be read.".into(),
            ));
        }
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "addon"
                    && stack.last().map(String::as_str) == Some("addons")
                    && build.is_none()
                {
                    build = Some(IndexBuild::begin(&event));
                } else if let Some(current) = build.as_mut() {
                    match name.as_str() {
                        "import" => current.push_import(&event),
                        "extension" => current.push_point(&event),
                        "provides" => current.begin_provides(),
                        "summary" => current.begin_summary(&event),
                        "broken" => current.capture = IndexCapture::Broken,
                        _ => {}
                    }
                }
                stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(event.name().as_ref());
                if let Some(current) = build.as_mut() {
                    match name.as_str() {
                        "import" => current.push_import(&event),
                        "extension" => current.push_point(&event),
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(event)) => {
                if let Some(current) = build.as_mut() {
                    if let Ok(text) = event.unescape() {
                        current.push_text(&text);
                    }
                }
            }
            Ok(Event::CData(event)) => {
                if let Some(current) = build.as_mut() {
                    current.push_text(&String::from_utf8_lossy(&event));
                }
            }
            Ok(Event::End(_)) => {
                let ended = stack.pop();
                if let Some(current) = build.as_mut() {
                    if matches!(ended.as_deref(), Some("provides" | "summary" | "broken")) {
                        current.capture = IndexCapture::None;
                    }
                }
                if ended.as_deref() == Some("addon")
                    && stack.last().map(String::as_str) == Some("addons")
                {
                    if let Some(current) = build.take() {
                        if let Some(package) = current.finish(datadir) {
                            packages.push(package);
                            if packages.len() >= PACKAGE_CAP {
                                break;
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => {
                return Err(EngineError::Message(
                    "That repository could not be read.".into(),
                ))
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(packages)
}

enum IndexCapture {
    None,
    Provides,
    Summary,
    Broken,
}

struct IndexBuild {
    id: String,
    name: String,
    version: String,
    summary: String,
    summary_english: bool,
    broken: String,
    points: Vec<String>,
    provides: String,
    imports: Vec<Import>,
    capture: IndexCapture,
    skip: bool,
}

impl IndexBuild {
    fn begin(event: &quick_xml::events::BytesStart<'_>) -> Self {
        let id = attr_value(event, "id");
        let skip = require_addon_id(&id).is_err();
        Self {
            id,
            name: attr_value(event, "name"),
            version: attr_value(event, "version"),
            summary: String::new(),
            summary_english: false,
            broken: String::new(),
            points: Vec::new(),
            provides: String::new(),
            imports: Vec::new(),
            capture: IndexCapture::None,
            skip,
        }
    }

    fn push_import(&mut self, event: &quick_xml::events::BytesStart<'_>) {
        if self.skip {
            return;
        }
        let id = attr_value(event, "addon");
        if id.is_empty() || id.starts_with("xbmc.") || require_addon_id(&id).is_err() {
            return;
        }
        let optional = attr_value(event, "optional").eq_ignore_ascii_case("true");
        self.imports.push(Import {
            addon: id,
            optional,
        });
    }

    fn push_point(&mut self, event: &quick_xml::events::BytesStart<'_>) {
        if self.skip {
            return;
        }
        let point = attr_value(event, "point");
        if !point.is_empty() {
            self.points.push(point);
        }
    }

    fn begin_provides(&mut self) {
        if self.skip {
            return;
        }
        // A second extension can say `executable` after `video`. Joining
        // those words keeps each one a separate token.
        if !self.provides.is_empty()
            && !self
                .provides
                .ends_with(|ch: char| ch.is_whitespace() || ch == ',')
        {
            self.provides.push(' ');
        }
        self.capture = IndexCapture::Provides;
    }

    fn begin_summary(&mut self, event: &quick_xml::events::BytesStart<'_>) {
        if self.skip {
            return;
        }
        let lang = attr_value(event, "lang").to_ascii_lowercase();
        let english = lang.is_empty() || lang.starts_with("en");
        if self.summary_english {
            self.capture = IndexCapture::None;
            return;
        }
        if english {
            self.summary.clear();
            self.summary_english = true;
            self.capture = IndexCapture::Summary;
            return;
        }
        if self.summary.is_empty() {
            self.capture = IndexCapture::Summary;
        } else {
            self.capture = IndexCapture::None;
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.skip {
            return;
        }
        match self.capture {
            IndexCapture::Provides if self.provides.len() < 80 => self.provides.push_str(text),
            IndexCapture::Summary if self.summary.len() < 240 => self.summary.push_str(text),
            IndexCapture::Broken if self.broken.len() < 200 => self.broken.push_str(text),
            _ => {}
        }
    }

    fn finish(self, datadir: &str) -> Option<Package> {
        if self.skip || !self.broken.trim().is_empty() {
            return None;
        }
        if !keep_in_index(&self.points, &self.provides) {
            return None;
        }
        let zip = package_url(datadir, &self.id, &self.version)?;
        let name = collapse(&self.name, 200);
        let name = if name.is_empty() {
            self.id.clone()
        } else {
            name
        };
        Some(Package {
            id: self.id,
            name,
            version: self.version,
            summary: collapse(&self.summary, 240),
            zip,
            video: is_video(&self.points, &self.provides),
            module: is_module(&self.points),
            imports: self.imports,
        })
    }
}

fn missing_dep(id: &str) -> String {
    format!("{id} is not in an installed repository.")
}

fn plan_deps(
    packages: &[Package],
    target: &str,
    installed: &dyn Fn(&str) -> bool,
) -> Result<Vec<String>, EngineError> {
    let mut order = Vec::new();
    let mut visiting = HashSet::new();
    let mut done = HashSet::new();
    fn walk(
        packages: &[Package],
        id: &str,
        depth: usize,
        installed: &dyn Fn(&str) -> bool,
        visiting: &mut HashSet<String>,
        done: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        if done.contains(id) || installed(id) {
            return Ok(());
        }
        if depth > DEP_DEPTH {
            return Err(EngineError::Message(
                "Could not install this add-on.".into(),
            ));
        }
        if !visiting.insert(id.to_string()) {
            return Ok(());
        }
        let Some(package) = packages.iter().find(|package| package.id == id) else {
            return Err(EngineError::Message(missing_dep(id)));
        };
        for import in &package.imports {
            if import.optional {
                continue;
            }
            if installed(&import.addon) {
                continue;
            }
            if !packages.iter().any(|package| package.id == import.addon) {
                return Err(EngineError::Message(missing_dep(&import.addon)));
            }
            walk(
                packages,
                &import.addon,
                depth + 1,
                installed,
                visiting,
                done,
                order,
            )?;
        }
        visiting.remove(id);
        done.insert(id.to_string());
        order.push(id.to_string());
        Ok(())
    }
    walk(
        packages,
        target,
        0,
        installed,
        &mut visiting,
        &mut done,
        &mut order,
    )?;
    Ok(order)
}

fn inflate_if_gzip(bytes: &[u8]) -> Result<Vec<u8>, EngineError> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        return gunzip(bytes);
    }
    Ok(bytes.to_vec())
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, EngineError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let read = decoder
            .read(&mut buf)
            .map_err(|_| EngineError::Message("That repository could not be read.".into()))?;
        if read == 0 {
            break;
        }
        if out.len().saturating_add(read) > INDEX_CAP {
            return Err(EngineError::Message(
                "That repository could not be read.".into(),
            ));
        }
        out.extend_from_slice(&buf[..read]);
    }
    Ok(out)
}

/// An add-on index starts with an `addons` element. A web page also starts
/// with `<`, and treating that page as the index hides every other directory.
fn looks_like_xml(bytes: &[u8]) -> bool {
    let mut index = 0;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        index = 3;
    }
    loop {
        index = skip_ascii_space(bytes, index);
        if index >= bytes.len() || bytes[index] != b'<' {
            return false;
        }
        if bytes[index..].starts_with(b"<?") {
            let Some(end) = find_bytes(&bytes[index + 2..], b"?>") else {
                return false;
            };
            index += 2 + end + 2;
            continue;
        }
        if bytes[index..].starts_with(b"<!--") {
            let Some(end) = find_bytes(&bytes[index + 4..], b"-->") else {
                return false;
            };
            index += 4 + end + 3;
            continue;
        }
        let start = index + 1;
        if start >= bytes.len() || matches!(bytes[start], b'!' | b'/' | b'?') {
            return false;
        }
        let mut end = start;
        while end < bytes.len() && is_xml_name(bytes[end]) {
            end += 1;
        }
        if end == start {
            return false;
        }
        let name = &bytes[start..end];
        let local = match name.iter().rposition(|byte| *byte == b':') {
            Some(at) => &name[at + 1..],
            None => name,
        };
        return local.eq_ignore_ascii_case(b"addons");
    }
}

fn skip_ascii_space(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b'\n' | b'\r') {
        index += 1;
    }
    index
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn is_xml_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')
}

/// One stray byte in one add-on's summary must not hide the whole
/// repository. Kodi reads such an index, so invalid bytes become U+FFFD here.
fn utf8_xml(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn collapse(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut spaced = false;
    for ch in text.chars() {
        if out.chars().count() >= max {
            break;
        }
        if ch.is_control() || ch.is_whitespace() {
            if !out.is_empty() {
                spaced = true;
            }
            continue;
        }
        if spaced {
            out.push(' ');
            spaced = false;
        }
        out.push(ch);
    }
    out
}

fn local_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.rsplit('}').next().unwrap_or(text.as_ref());
    text.rsplit(':').next().unwrap_or(text).to_string()
}

fn attr_value(event: &quick_xml::events::BytesStart<'_>, key: &str) -> String {
    event
        .attributes()
        .filter_map(|attr| attr.ok())
        .find(|attr| local_name(attr.key.as_ref()) == key)
        .map(|attr| {
            attr.unescape_value()
                .map(|text| text.trim().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn under(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    if rel.is_empty() || rel.len() > 240 {
        return None;
    }
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') || part.len() > 180
        {
            return None;
        }
        out.push(part);
    }
    if out.starts_with(root) {
        Some(out)
    } else {
        None
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), ()> {
    fs::create_dir_all(to).map_err(|_| ())?;
    for entry in fs::read_dir(from).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let file_type = entry.file_type().map_err(|_| ())?;
        let dest = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), dest).map_err(|_| ())?;
        }
    }
    Ok(())
}

fn read_capped(path: &Path, cap: usize) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = file.take(cap as u64 + 1);
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).ok()?;
    if buf.len() > cap {
        return None;
    }
    // Read the way a zip's addon.xml is, so a stray byte in a name does not
    // hide an installed repository.
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn reserved_addon_dir(id: &str) -> bool {
    matches!(id.to_ascii_lowercase().as_str(), "packages" | "temp")
}

fn direct_child(parent: &Path, child: &Path, id: &str) -> bool {
    child.parent() == Some(parent) && child.file_name().and_then(|name| name.to_str()) == Some(id)
}

fn read_profile_kind(dir: &Path, id: &str) -> Result<(String, DiskKind), EngineError> {
    let xml = read_capped(&dir.join("addon.xml"), XML_CAP)
        .ok_or_else(|| EngineError::Message("That add-on could not be removed.".into()))?;
    let meta = parse_addon_xml(&xml)
        .map_err(|_| EngineError::Message("That add-on could not be removed.".into()))?;
    if meta.id != id {
        return Err(EngineError::Message(
            "That add-on could not be removed.".into(),
        ));
    }
    let name = collapse(&meta.name, 200);
    let name = if name.is_empty() {
        id.to_string()
    } else {
        name
    };
    if meta
        .points
        .iter()
        .any(|point| point == "xbmc.addon.repository")
    {
        return Ok((name, DiskKind::Repository));
    }
    if is_video(&meta.points, &meta.provides) {
        return Ok((name, DiskKind::Video));
    }
    if is_module(&meta.points) {
        return Ok((name, DiskKind::Module));
    }
    Err(EngineError::BadRequest(
        "That add-on cannot be removed from here.".into(),
    ))
}

fn wrong_kind(found: DiskKind, expect: DiskKind) -> EngineError {
    match (expect, found) {
        (DiskKind::Repository, DiskKind::Video) => {
            EngineError::BadRequest("That is a video add-on, not a repository.".into())
        }
        (DiskKind::Video, DiskKind::Repository) => {
            EngineError::BadRequest("That is a repository, not a video add-on.".into())
        }
        _ => EngineError::BadRequest("That add-on cannot be removed from here.".into()),
    }
}

fn marker_is(path: &Path, expect: DiskKind) -> bool {
    fs::read_to_string(path)
        .map(|text| text.trim() == expect.as_str())
        .unwrap_or(false)
}

const REMOVE_WALK_CAP: usize = 20_000;

fn guard_tree(path: &Path) -> Result<(), EngineError> {
    let mut seen = 0usize;
    if blocked_tree(path, &mut seen) {
        Err(EngineError::Message(
            "That add-on could not be removed.".into(),
        ))
    } else {
        Ok(())
    }
}

/// A reparse point (a Windows junction or a symlink) is not followed.
/// Hitting the walk cap also refuses, so a link past the cap is not deleted.
fn blocked_tree(path: &Path, seen: &mut usize) -> bool {
    if *seen >= REMOVE_WALK_CAP {
        return true;
    }
    *seen += 1;
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
        Ok(meta) => {
            if is_reparse(&meta) {
                return true;
            }
            if !meta.is_dir() {
                return false;
            }
            let Ok(entries) = fs::read_dir(path) else {
                return true;
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    return true;
                };
                if blocked_tree(&entry.path(), seen) {
                    return true;
                }
            }
            false
        }
    }
}

fn is_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn file_busy(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(err.raw_os_error(), Some(32) | Some(33))
}

fn delete_tree(path: &Path) -> Result<(), EngineError> {
    if matches!(fs::symlink_metadata(path), Err(err) if err.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(());
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if file_busy(&err) => Err(EngineError::Message(
            "That add-on is in use. Stop playback, then remove it again.".into(),
        )),
        Err(_) => Err(EngineError::Message(
            "That add-on could not be removed.".into(),
        )),
    }
}

fn package_owned(file_name: &str, id: &str) -> bool {
    let prefix = format!("{id}-");
    let Some(rest) = file_name.strip_prefix(&prefix) else {
        return false;
    };
    let Some(version) = rest.strip_suffix(".zip") else {
        return false;
    };
    valid_version(version)
}

fn guard_packages(dir: &Path, id: &str) -> Result<(), EngineError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(EngineError::Message(
                "That add-on could not be removed.".into(),
            ))
        }
    };
    let mut seen = 0usize;
    for entry in entries {
        seen += 1;
        if seen > SCAN_CAP {
            break;
        }
        let Ok(entry) = entry else {
            return Err(EngineError::Message(
                "That add-on could not be removed.".into(),
            ));
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !package_owned(&name, id) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            return Err(EngineError::Message(
                "That add-on could not be removed.".into(),
            ));
        };
        if is_reparse(&meta) || !meta.is_file() {
            return Err(EngineError::Message(
                "That add-on could not be removed.".into(),
            ));
        }
    }
    Ok(())
}

fn delete_packages(dir: &Path, id: &str) -> Result<(), EngineError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(EngineError::Message(
                "That add-on could not be removed.".into(),
            ))
        }
    };
    let mut seen = 0usize;
    for entry in entries {
        seen += 1;
        if seen > SCAN_CAP {
            break;
        }
        let Ok(entry) = entry else {
            return Err(EngineError::Message(
                "That add-on could not be removed.".into(),
            ));
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !package_owned(&name, id) {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) if file_busy(&err) => {
                return Err(EngineError::Message(
                    "That add-on is in use. Stop playback, then remove it again.".into(),
                ))
            }
            Err(_) => {
                return Err(EngineError::Message(
                    "That add-on could not be removed.".into(),
                ))
            }
        }
    }
    Ok(())
}

fn scan_repos(dir: &Path, removable: bool, seen: &mut HashSet<String>, items: &mut Vec<RepoCard>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if items.len() >= REPO_CAP || seen.len() >= SCAN_CAP {
            break;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if require_addon_id(id).is_err() || seen.contains(id) {
            continue;
        }
        let Some(xml) = read_capped(&path.join("addon.xml"), XML_CAP) else {
            continue;
        };
        seen.insert(id.to_string());
        if let Some(card) = repo_card(&xml, id, removable) {
            items.push(card);
        }
    }
}

fn repo_card(xml: &str, id: &str, removable: bool) -> Option<RepoCard> {
    let meta = parse_addon_xml(xml).ok()?;
    if meta.id != id
        || !meta
            .points
            .iter()
            .any(|point| point == "xbmc.addon.repository")
    {
        return None;
    }
    let name = collapse(&meta.name, 200);
    let name = if name.is_empty() {
        id.to_string()
    } else {
        name
    };
    Some(RepoCard {
        id: id.to_string(),
        name,
        removable,
    })
}

fn read_repo_xml(
    profile: &Path,
    programs: &Path,
    id: &str,
) -> Result<(String, String), EngineError> {
    let profile_xml = profile.join("addons").join(id).join("addon.xml");
    let programs_xml = programs.join(id).join("addon.xml");
    let path = if profile_xml.is_file() {
        profile_xml
    } else if programs_xml.is_file() {
        programs_xml
    } else {
        return Err(EngineError::NotFound);
    };
    let xml = read_capped(&path, XML_CAP)
        .ok_or_else(|| EngineError::Message("That repository could not be read.".into()))?;
    let meta = parse_addon_xml(&xml)?;
    if meta.id != id
        || !meta
            .points
            .iter()
            .any(|point| point == "xbmc.addon.repository")
    {
        return Err(EngineError::Message(
            "That repository could not be read.".into(),
        ));
    }
    let name = collapse(&meta.name, 200);
    let name = if name.is_empty() {
        id.to_string()
    } else {
        name
    };
    Ok((name, xml))
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/addons.rs"]
mod tests;
