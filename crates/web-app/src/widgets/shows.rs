//! What Portal knows about the shows a page lists, as main.ts and api.ts keep it: which
//! folder is which season of which show (api.ts's season memory, `portal-seasons`), the
//! shows opened from Browse, the library's detail for each show, and from those, whether a
//! folder is a show's page or one of its seasons ([`page_show`], [`show_view`]).
//!
//! The old page keeps the season memory in `sessionStorage` under `portal-seasons`, so it
//! lasts as long as the tab. ENGINE GAP: RemedyUI cannot read or write a page's
//! sessionStorage (remedy-ui-services-webstorage reads localStorage only, src/browser.rs),
//! so here it lasts as long as the page, which is what the old page falls back to when its
//! storage is refused ("The labels still last for this page view", api.ts `saveMemory`).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use portal_api::{BrowseItem, BrowsePage, Episode, ItemKind, MediaInfo, MediaKind, SeasonFolder, TitleDetail, TitleKind};

use crate::widgets::art::poster_from_id;
use crate::widgets::saved::SaveTitle;
use crate::widgets::titles::{clean_episode_title, episode_code};

#[derive(Default)]
struct Memory {
    season_to_show: HashMap<String, String>,
    season_number: HashMap<String, u32>,
    show_name: HashMap<String, String>,
    labels: HashMap<String, String>,
}

/// A show folder opened from Browse (main.ts `ShowRef`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShowRef {
    pub id: String,
    pub name: String,
    pub art: Option<String>,
    pub info: Option<MediaInfo>,
}

thread_local! {
    static MEMORY: RefCell<Memory> = RefCell::default();
    static OPENED: RefCell<HashMap<String, ShowRef>> = RefCell::default();
    /// Library detail for shows this page looked up; `None` when the library has no such show.
    static LIBRARY: RefCell<HashMap<String, Option<TitleDetail>>> = RefCell::default();
}

fn mem<R>(f: impl FnOnce(&mut Memory) -> R) -> R {
    MEMORY.with(|m| f(&mut m.borrow_mut()))
}

/// `^(?:season|series)\s+(\d+)\b`, case-insensitive, on the trimmed label.
fn season_label_number(label: &str) -> Option<u32> {
    let t = label.trim();
    let lower = t.to_ascii_lowercase();
    let rest = lower.strip_prefix("season").or_else(|| lower.strip_prefix("series"))?;
    let digits_at = rest.len() - rest.trim_start().len();
    if digits_at == 0 {
        return None;
    }
    let rest = &rest[digits_at..];
    let n = rest.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    // `\b` after the digits: the next character is not a word character.
    if rest[n..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    rest[..n].parse().ok()
}

/// The memory as the old page writes it under `portal-seasons` (api.ts `SeasonMemory`):
/// `{"seasonToShow": {}, "seasonNumber": {}, "showName": {}, "labels": {}}`.
pub fn memory_json() -> String {
    mem(|m| {
        serde_json::json!({
            "seasonToShow": m.season_to_show,
            "seasonNumber": m.season_number,
            "showName": m.show_name,
            "labels": m.labels,
        })
        .to_string()
    })
}

/// Loads what the old page saved (api.ts `loadMemory`): a part that is missing or not an
/// object of the right values is left empty.
pub fn load_memory_json(text: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return };
    fn map<T: serde::de::DeserializeOwned + Default>(v: &serde_json::Value, key: &str) -> T {
        v.get(key).and_then(|m| serde_json::from_value(m.clone()).ok()).unwrap_or_default()
    }
    mem(|m| {
        m.season_to_show = map(&v, "seasonToShow");
        m.season_number = map(&v, "seasonNumber");
        m.show_name = map(&v, "showName");
        m.labels = map(&v, "labels");
    });
}

pub fn is_season_label(label: &str) -> bool {
    season_label_number(label).is_some()
}

pub fn note_path_label(path: &str, label: &str) {
    if path.is_empty() || label.trim().is_empty() {
        return;
    }
    mem(|m| m.labels.insert(path.to_owned(), label.trim().to_owned()));
}

pub fn path_label(path: &str) -> String {
    mem(|m| m.labels.get(path).cloned().unwrap_or_default())
}

pub fn remembered_name(id: &str) -> String {
    mem(|m| m.show_name.get(id).cloned().unwrap_or_default())
}

/// What a folder's items say about its show and seasons (api.ts `rememberFolder`).
pub fn remember_folder(folder: &str, items: &[BrowseItem]) {
    mem(|m| {
        if let Some(t) = items.iter().find_map(|i| i.show_title.clone().filter(|s| !s.is_empty())) {
            m.show_name.insert(folder.to_owned(), t);
        }
        if !m.show_name.contains_key(folder) {
            if let Some(own) = m.labels.get(folder).cloned().filter(|o| !is_season_label(o)) {
                m.show_name.insert(folder.to_owned(), own);
            }
        }
        for item in items {
            let Some(n) = season_label_number(&item.label) else { continue };
            if item.kind != ItemKind::Open || item.path.is_empty() {
                continue;
            }
            m.season_to_show.insert(item.path.clone(), folder.to_owned());
            m.season_number.insert(item.path.clone(), n);
            if let Some(name) = m.show_name.get(folder).cloned() {
                m.show_name.insert(item.path.clone(), name);
            }
        }
    });
}

/// The library's season folders for a show (api.ts `rememberShowSeasons`).
pub fn remember_show_seasons(show_id: &str, name: &str, folders: &[SeasonFolder]) {
    if show_id.is_empty() {
        return;
    }
    let named = Some(name.trim()).filter(|n| !n.is_empty() && !is_season_label(n)).map(str::to_owned);
    mem(|m| {
        if let Some(n) = &named {
            m.show_name.insert(show_id.to_owned(), n.clone());
        }
        for f in folders {
            if f.path.is_empty() || f.path == show_id {
                continue;
            }
            m.season_to_show.insert(f.path.clone(), show_id.to_owned());
            m.season_number.insert(f.path.clone(), f.season);
            if let Some(n) = &named {
                m.show_name.insert(f.path.clone(), n.clone());
            }
            m.labels.entry(f.path.clone()).or_insert_with(|| season_name(f.season));
        }
    });
}

/// The season folders this page has seen for a show, lowest season first.
pub fn season_folders_for(show_id: &str) -> Vec<SeasonFolder> {
    let mut out: Vec<SeasonFolder> = mem(|m| {
        m.season_to_show
            .iter()
            .filter(|(_, show)| *show == show_id)
            .filter_map(|(path, _)| m.season_number.get(path).map(|s| SeasonFolder { season: *s, path: path.clone() }))
            .collect()
    });
    out.sort_by(|a, b| a.season.cmp(&b.season).then_with(|| a.path.cmp(&b.path)));
    out
}

pub fn season_number(folder: &str) -> Option<u32> {
    mem(|m| m.season_number.get(folder).copied())
}

pub fn season_parent(folder: &str) -> String {
    mem(|m| m.season_to_show.get(folder).cloned().unwrap_or_default())
}

/// api.ts `showFor`.
pub fn show_for(folder: &str, item: &BrowseItem) -> Option<(String, Option<u32>)> {
    let (mapped, known) = mem(|m| (m.season_to_show.get(folder).cloned(), m.season_number.get(folder).copied()));
    let season = item.season.or(known);
    if let Some(id) = mapped {
        return Some((id, season));
    }
    let tv = item.title_kind == Some(TitleKind::Tv) || season.is_some();
    match (tv, season, item.episode) {
        (true, Some(s), Some(e)) if e > 0 => Some((folder.to_owned(), Some(s))),
        _ => None,
    }
}

/// `URLSearchParams` over the part after `?`: pairs split on `&` and `=`, `+` a space,
/// percent escapes decoded (a bad escape stays as written).
pub fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (form_decode(k), form_decode(v))
        })
        .collect()
}

fn form_decode(s: &str) -> String {
    percent_decode(&s.replace('+', " ")).unwrap_or_else(|| s.replace('+', " "))
}

/// `decodeURIComponent`: `None` on a malformed escape or bytes that are not UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Text a person would read: not JSON, a URL, a bare number, or a placeholder.
fn human_part(value: &str) -> bool {
    let t = value.trim();
    let lower = t.to_ascii_lowercase();
    let url = t.find("://").is_some_and(|at| {
        let scheme = &t[..at];
        scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "_+.-".contains(c))
    });
    !t.is_empty()
        && t.chars().count() <= 120
        && !t.starts_with(['[', '{'])
        && !url
        && !t.bytes().all(|b| b.is_ascii_digit())
        && !matches!(lower.as_str(), "none" | "null" | "true" | "false" | "undefined")
}

/// A heading for a folder Portal has no label for, guessed from its path (api.ts `folderLabel`).
pub fn folder_label(path: &str) -> String {
    if let Some((_, query)) = path.split_once('?') {
        let params = query_pairs(query);
        let season = params.iter().find(|(k, _)| k.eq_ignore_ascii_case("season")).map(|(_, v)| v.as_str()).unwrap_or("");
        let episode = params.iter().any(|(k, _)| k.eq_ignore_ascii_case("episode"));
        if !season.is_empty() && season.bytes().all(|b| b.is_ascii_digit()) && !episode {
            let n: u64 = season.parse().unwrap_or(0);
            if n > 0 {
                return format!("Season {n}");
            }
        }
        let named = params.iter().find(|(k, v)| {
            let k = k.to_ascii_lowercase();
            (k.contains("name") || k.contains("title")) && human_part(v)
        });
        if let Some((_, v)) = named {
            return v.trim().to_owned();
        }
        if let Some((_, v)) = params.iter().rev().find(|(_, v)| human_part(v)) {
            return v.trim().to_owned();
        }
    }
    let base = path.split_once('?').map_or(path, |(b, _)| b);
    match base.split('/').filter(|p| !p.is_empty() && *p != "plugin:").last() {
        Some(last) => percent_decode(last).unwrap_or_else(|| last.to_owned()),
        None => path.to_owned(),
    }
}

/// "Season 2", or "Specials" for season 0.
pub fn season_name(season: u32) -> String {
    if season == 0 { "Specials".into() } else { format!("Season {season}") }
}

// ---- settings-link.ts `settingsAddon` ----

fn plugin_at(file: &str) -> Option<usize> {
    file.as_bytes().windows(9).position(|w| w.eq_ignore_ascii_case(b"plugin://"))
}

fn stop_builtin(c: char) -> bool {
    matches!(c, ')' | '\'' | '"')
}

/// The add-on whose own settings a row opens through `openSettings`.
pub fn settings_addon(file: &str) -> Option<String> {
    let at = plugin_at(file)?;
    let span = &file[at..];
    let host: String = span[9..].chars().take_while(|c| *c != '/' && *c != '?' && !stop_builtin(*c)).collect();
    let q = span.find(|c: char| c == '?' || stop_builtin(c)).filter(|i| span[*i..].starts_with('?'))? + 1;
    let end = span[q..].find(stop_builtin).map_or(span.len(), |i| q + i);
    let action = span[q..end].split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        k.eq_ignore_ascii_case("action").then(|| v.trim().to_owned())
    })?;
    if !action.eq_ignore_ascii_case("openSettings") {
        return None;
    }
    let ok = !host.is_empty()
        && host.len() <= 80
        && !host.contains("..")
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    ok.then_some(host)
}

pub fn is_settings_item(item: &BrowseItem) -> bool {
    item.settings_addon.as_deref().is_some_and(|s| !s.is_empty()) || settings_addon(&item.path).is_some()
}

// ---- main.ts: what an item is ----

/// A browse item Portal can save or look up: a movie, an episode (saved as its show), or a
/// show folder (main.ts `Media`).
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub kind: MediaKind,
    pub save: SaveTitle,
    pub seed: MediaInfo,
    pub art: Option<String>,
}

/// The show an episode item belongs to (main.ts `episodeShow`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpisodeOf {
    pub id: String,
    pub name: String,
    pub season: Option<u32>,
    pub episode: u32,
}

pub fn opened_show(id: &str) -> Option<ShowRef> {
    OPENED.with(|o| o.borrow().get(id).cloned())
}

pub fn note_opened_show(path: &str, show: ShowRef) {
    OPENED.with(|o| o.borrow_mut().insert(path.to_owned(), show));
}

pub fn show_name(id: &str, hint: Option<&str>) -> String {
    if let Some(h) = hint.map(str::trim).filter(|h| !h.is_empty()) {
        return h.to_owned();
    }
    if let Some(n) = opened_show(id).map(|s| s.name).filter(|n| !n.is_empty()) {
        return n;
    }
    let known = remembered_name(id);
    if !known.is_empty() && !is_season_label(&known) {
        return known;
    }
    let label = path_label(id);
    if !label.is_empty() && !is_season_label(&label) {
        return label;
    }
    folder_label(id)
}

fn info_of(item: &BrowseItem) -> Option<&MediaInfo> {
    item.info.as_ref()
}

pub fn episode_show(folder: &str, item: &BrowseItem) -> Option<EpisodeOf> {
    let info = info_of(item);
    let episode = item.episode.or(info.and_then(|i| i.episode)).filter(|e| *e > 0)?;
    let show = show_for(folder, item);
    let show_path = item.show_path.clone().filter(|p| !p.is_empty());
    if show.is_none() && show_path.is_none() && info.and_then(|i| i.kind) != Some(MediaKind::Episode) {
        return None;
    }
    let parent = Some(season_parent(folder)).filter(|p| !p.is_empty());
    let id = show_path.or(show.as_ref().map(|s| s.0.clone())).or(parent).unwrap_or_else(|| folder.to_owned());
    let hint = item.show_title.clone().filter(|s| !s.is_empty()).or(info.and_then(|i| i.show_title.clone()));
    Some(EpisodeOf {
        name: show_name(&id, hint.as_deref()),
        season: show.and_then(|s| s.1).or(item.season).or(info.and_then(|i| i.season)),
        episode,
        id,
    })
}

fn nonempty(s: Option<&String>) -> Option<String> {
    s.filter(|s| !s.is_empty()).cloned()
}

fn movie_media(item: &BrowseItem) -> Media {
    let info = info_of(item);
    let name = nonempty(info.and_then(|i| i.title.as_ref())).unwrap_or_else(|| item.label.clone());
    let mut seed = info.cloned().unwrap_or_default();
    seed.kind = Some(MediaKind::Movie);
    seed.title = Some(name.clone());
    Media {
        kind: MediaKind::Movie,
        save: SaveTitle {
            id: item.path.clone(),
            kind: TitleKind::Movie,
            name,
            art: item.art.clone(),
            year: info.and_then(|i| i.year),
            imdb_id: info.and_then(|i| i.imdb_id.clone()),
        },
        seed,
        art: item.art.clone(),
    }
}

/// main.ts `mediaOf`.
pub fn media_of(folder: &str, item: &BrowseItem) -> Option<Media> {
    if item.path.is_empty() || is_settings_item(item) || item.kind == ItemKind::Run {
        return None;
    }
    let info = info_of(item);
    if item.kind == ItemKind::Play {
        if let Some(show) = episode_show(folder, item) {
            let opened = opened_show(&show.id);
            let mut seed = info.cloned().unwrap_or_default();
            seed.kind = Some(MediaKind::Episode);
            seed.title = Some(
                nonempty(info.and_then(|i| i.title.as_ref()))
                    .or_else(|| {
                        Some(clean_episode_title(&item.label, show.season, Some(show.episode), Some(&show.name)))
                            .filter(|s| !s.is_empty())
                    })
                    .unwrap_or_else(|| item.label.clone()),
            );
            seed.show_title = Some(nonempty(info.and_then(|i| i.show_title.as_ref())).unwrap_or_else(|| show.name.clone()));
            seed.season = show.season;
            seed.episode = Some(show.episode);
            // The episode's own art is a still; the show's poster suits the watchlist better.
            let art = opened.and_then(|o| o.art).or(info.and_then(|i| i.poster.clone())).or(item.art.clone());
            return Some(Media {
                kind: MediaKind::Episode,
                save: SaveTitle { id: show.id, kind: TitleKind::Tv, name: show.name, art, year: None, imdb_id: None },
                seed,
                art: item.art.clone(),
            });
        }
        // A TV item Portal cannot tie to a show has nothing to save.
        if item.title_kind == Some(TitleKind::Tv) && info.and_then(|i| i.kind) != Some(MediaKind::Movie) {
            return None;
        }
        return Some(movie_media(item));
    }
    if item.kind != ItemKind::Open || is_season_label(&item.label) {
        return None;
    }
    let kind = info.and_then(|i| i.kind).map(|k| match k {
        MediaKind::Movie => TitleKind::Movie,
        _ => TitleKind::Tv,
    });
    // `info.kind ?? item.title_kind`: an episode kind is neither "tv" nor "movie" there.
    let episode_kind = info.and_then(|i| i.kind) == Some(MediaKind::Episode);
    let kind = if episode_kind { None } else { kind.or(item.title_kind) };
    if kind == Some(TitleKind::Tv) && item.season.is_none() && item.episode.is_none() {
        let id = nonempty(item.show_path.as_ref()).unwrap_or_else(|| item.path.clone());
        let name = nonempty(item.show_title.as_ref())
            .or(nonempty(info.and_then(|i| i.show_title.as_ref())))
            .or(nonempty(info.and_then(|i| i.title.as_ref())))
            .unwrap_or_else(|| item.label.clone());
        let mut seed = info.cloned().unwrap_or_default();
        seed.kind = Some(MediaKind::Tv);
        seed.title = Some(name.clone());
        return Some(Media {
            kind: MediaKind::Tv,
            save: SaveTitle {
                id,
                kind: TitleKind::Tv,
                name,
                art: item.art.clone(),
                year: info.and_then(|i| i.year),
                imdb_id: info.and_then(|i| i.imdb_id.clone()),
            },
            seed,
            art: item.art.clone(),
        });
    }
    if kind == Some(TitleKind::Movie) {
        return Some(movie_media(item));
    }
    None
}

pub fn show_media(r: &ShowRef) -> Media {
    let mut seed = r.info.clone().unwrap_or_default();
    seed.kind = Some(MediaKind::Tv);
    seed.title = Some(r.name.clone());
    Media {
        kind: MediaKind::Tv,
        save: SaveTitle {
            id: r.id.clone(),
            kind: TitleKind::Tv,
            name: r.name.clone(),
            art: r.art.clone(),
            year: r.info.as_ref().and_then(|i| i.year),
            imdb_id: r.info.as_ref().and_then(|i| i.imdb_id.clone()),
        },
        seed,
        art: r.art.clone(),
    }
}

/// The show a page belongs to: opened from a show folder, a season of one, or one show's
/// episodes (main.ts `pageShow`).
pub fn page_show(page: &BrowsePage) -> Option<Media> {
    if let Some(o) = opened_show(&page.path) {
        return Some(show_media(&o));
    }
    let parent = season_parent(&page.path);
    if !parent.is_empty() {
        let r = opened_show(&parent).unwrap_or_else(|| ShowRef { name: show_name(&parent, None), id: parent, ..ShowRef::default() });
        return Some(show_media(&r));
    }
    let episodes: Vec<Media> =
        page.items.iter().filter_map(|i| media_of(&page.path, i)).filter(|m| m.kind == MediaKind::Episode).collect();
    if let Some(first) = episodes.first().filter(|f| episodes.iter().all(|m| m.save.id == f.save.id)) {
        let f = &first.save;
        let r = opened_show(&f.id).unwrap_or_else(|| ShowRef { id: f.id.clone(), name: f.name.clone(), art: f.art.clone(), info: None });
        return Some(show_media(&r));
    }
    let seasons = page.items.iter().filter(|i| i.kind == ItemKind::Open && is_season_label(&i.label)).count();
    let others = page.items.iter().filter(|i| !is_settings_item(i) && i.kind != ItemKind::Run).count();
    if seasons > 0 && seasons * 2 >= others {
        return Some(show_media(&ShowRef { id: page.path.clone(), name: show_name(&page.path, None), ..ShowRef::default() }));
    }
    None
}

// ---- the library's shows ----

pub fn library_show(id: &str) -> Option<TitleDetail> {
    LIBRARY.with(|l| l.borrow().get(id).cloned().flatten())
}

/// True once the library was asked about `id` (answered with the show or with none).
pub fn library_known(id: &str) -> bool {
    LIBRARY.with(|l| l.borrow().contains_key(id))
}

pub fn note_library_missing(id: &str) {
    LIBRARY.with(|l| l.borrow_mut().insert(id.to_owned(), None));
}

/// The library's show, filed so its season pages know the show and its art (main.ts
/// `noteLibraryShow`).
pub fn note_library_show(title: &TitleDetail) {
    LIBRARY.with(|l| l.borrow_mut().insert(title.id.clone(), Some(title.clone())));
    if title.kind != TitleKind::Tv {
        return;
    }
    remember_show_seasons(&title.id, &title.name, &title.season_folders);
    let opened = opened_show(&title.id);
    let name = opened.as_ref().map(|o| o.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| title.name.clone());
    // The library keeps the latest episode's still as the show's art; a poster reads better.
    let art = opened.as_ref().and_then(|o| o.art.clone()).or_else(|| poster_from_id(&title.id)).or(title.art.clone());
    let info = opened.and_then(|o| o.info).or_else(|| {
        Some(MediaInfo {
            kind: Some(MediaKind::Tv),
            title: Some(title.name.clone()),
            year: title.year,
            imdb_id: title.imdb_id.clone(),
            ..MediaInfo::default()
        })
    });
    note_opened_show(&title.id, ShowRef { id: title.id.clone(), name, art, info });
}

/// The episode to resume or watch next, once something of the show was played.
pub fn where_watching(title: &TitleDetail) -> Option<(u32, u32)> {
    let played = title.seasons.iter().any(|s| s.episodes.iter().any(|e| e.position > 0.0 || e.finished));
    let next = title.next.as_ref().filter(|_| played)?;
    Some((next.season, next.episode))
}

/// Every season folder known for the show, from the library and from this page.
pub fn show_seasons(show_id: &str) -> Vec<SeasonFolder> {
    let mut by: BTreeMap<u32, String> = BTreeMap::new();
    if let Some(t) = library_show(show_id) {
        for f in t.season_folders {
            by.insert(f.season, f.path);
        }
    }
    for f in season_folders_for(show_id) {
        by.entry(f.season).or_insert(f.path);
    }
    by.into_iter().map(|(season, path)| SeasonFolder { season, path }).collect()
}

/// The library's episode at `path`, else the one with that season and number.
pub fn library_episode<'a>(title: &'a TitleDetail, path: &str, season: Option<u32>, episode: u32) -> Option<&'a Episode> {
    let mut by_number = None;
    for e in title.seasons.iter().flat_map(|s| &s.episodes) {
        if e.path == path {
            return Some(e);
        }
        if by_number.is_none() && Some(e.season) == season && e.episode == episode {
            by_number = Some(e);
        }
    }
    by_number
}

/// A show's own page or one of its seasons, with what the library knows (main.ts `ShowView`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShowView {
    pub show: Media,
    pub library: Option<TitleDetail>,
    /// The season this page lists; `None` on the show's own page.
    pub season: Option<u32>,
    /// The page lists the show's episodes, so they show as rows.
    pub episodes: bool,
    /// The episode to resume or watch next.
    pub next: Option<(u32, u32)>,
}

/// main.ts `showView`.
pub fn show_view(page: &BrowsePage, show: Media) -> ShowView {
    let library = library_show(&show.save.id);
    let listed: Vec<&BrowseItem> = page.items.iter().filter(|i| !is_settings_item(i) && i.kind != ItemKind::Run).collect();
    let episodes: Vec<&&BrowseItem> =
        listed.iter().filter(|i| i.kind == ItemKind::Play && episode_show(&page.path, i).is_some()).collect();
    let mut seasons: Vec<u32> =
        episodes.iter().filter_map(|i| i.season.or(i.info.as_ref().and_then(|f| f.season))).collect();
    seasons.sort_unstable();
    seasons.dedup();
    let season = if page.path == show.save.id && episodes.is_empty() {
        None
    } else {
        season_number(&page.path)
            .or_else(|| {
                library.as_ref().and_then(|l| l.season_folders.iter().find(|f| f.path == page.path).map(|f| f.season))
            })
            .or(if seasons.len() == 1 { seasons.first().copied() } else { None })
    };
    ShowView {
        next: library.as_ref().and_then(where_watching),
        episodes: !episodes.is_empty() && episodes.len() * 2 >= listed.len(),
        season,
        library,
        show,
    }
}

/// How far into a season watching got, for its card on the show's page.
pub fn season_progress(view: &ShowView, season: u32) -> Option<(String, f64, bool)> {
    let eps: &[Episode] = view
        .library
        .as_ref()
        .and_then(|l| l.seasons.iter().find(|s| s.number == season))
        .map_or(&[], |s| &s.episodes);
    let current = view.next.is_some_and(|n| n.0 == season);
    let watched = eps.iter().filter(|e| e.finished).count();
    if let (true, Some((s, e))) = (current, view.next) {
        let f = if eps.is_empty() { 0.0 } else { watched as f64 / eps.len() as f64 };
        return Some((format!("Up next {}", episode_code(Some(s), Some(e))), f, true));
    }
    if watched == 0 {
        return None;
    }
    let note = if watched == eps.len() { "Watched".to_owned() } else { format!("{watched} of {} watched", eps.len()) };
    Some((note, watched as f64 / eps.len() as f64, current))
}

pub fn season_watched(library: Option<&TitleDetail>, season: u32) -> bool {
    library
        .and_then(|l| l.seasons.iter().find(|s| s.number == season))
        .is_some_and(|s| !s.episodes.is_empty() && s.episodes.iter().all(|e| e.finished))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, path: &str, kind: ItemKind) -> BrowseItem {
        BrowseItem {
            id: path.into(),
            label: label.into(),
            path: path.into(),
            kind,
            art: None,
            title_kind: None,
            season: None,
            episode: None,
            show_title: None,
            show_path: None,
            settings_addon: None,
            info: None,
        }
    }

    #[test]
    fn memory_round_trips_in_the_old_pages_format() {
        load_memory_json(r#"{"seasonToShow":{"s2":"show"},"seasonNumber":{"s2":2},"showName":{"show":"Heat"},"labels":7}"#);
        assert_eq!((season_parent("s2"), season_number("s2"), remembered_name("show")), ("show".into(), Some(2), "Heat".into()));
        let v: serde_json::Value = serde_json::from_str(&memory_json()).unwrap_or_default();
        assert_eq!(v["seasonNumber"]["s2"], 2);
        assert_eq!(v["labels"], serde_json::json!({}));
        load_memory_json("{}");
    }

    #[test]
    fn season_labels_match_the_regex() {
        assert_eq!(season_label_number(" Season 2 "), Some(2));
        assert_eq!(season_label_number("SERIES 10 (2019)"), Some(10));
        assert_eq!(season_label_number("Season2"), None);
        assert_eq!(season_label_number("Season 2x"), None);
        assert_eq!(season_label_number("Seasons 2"), None);
    }

    #[test]
    fn folder_labels_follow_api_ts() {
        assert_eq!(folder_label("plugin://p/?action=episodes&season=3"), "Season 3");
        assert_eq!(folder_label("plugin://p/?action=x&tvshowtitle=The+Wire&year=2002"), "The Wire");
        assert_eq!(folder_label("plugin://p/?action=x&q=%7B%7D&genre=Drama"), "Drama");
        assert_eq!(folder_label("plugin://plugin.video.x/Movies%20A/"), "Movies A");
    }

    #[test]
    fn settings_rows_are_spotted() {
        assert_eq!(settings_addon("plugin://plugin.video.x/?action=openSettings").as_deref(), Some("plugin.video.x"));
        assert_eq!(settings_addon("RunPlugin(plugin://plugin.video.x/?action=OPENSETTINGS)").as_deref(), Some("plugin.video.x"));
        assert_eq!(settings_addon("plugin://plugin.video.x/?action=play"), None);
        assert_eq!(settings_addon("plugin://a..b/?action=openSettings"), None);
    }

    #[test]
    fn a_season_folder_is_its_shows_page() {
        let show = "plugin://p/?action=seasons&tvshowtitle=Tracker";
        let s4 = "plugin://p/?action=episodes&season=4";
        remember_show_seasons(show, "Tracker", &[SeasonFolder { season: 4, path: s4.into() }]);
        let mut ep = item("1x01. Pilot", "plugin://p/?action=play&e=1", ItemKind::Play);
        ep.episode = Some(1);
        let page = BrowsePage { path: s4.into(), items: vec![ep.clone()] };
        let media = page_show(&page).unwrap_or_else(|| unreachable!("a season of a known show"));
        assert_eq!((media.save.id.as_str(), media.save.name.as_str()), (show, "Tracker"));
        let view = show_view(&page, media);
        assert_eq!((view.season, view.episodes), (Some(4), true));
        let m = media_of(s4, &ep).unwrap_or_else(|| unreachable!("an episode"));
        assert_eq!((m.kind, m.seed.season, m.seed.title.as_deref()), (MediaKind::Episode, Some(4), Some("Pilot")));
        let seasons = BrowsePage { path: show.into(), items: vec![item("Season 4", s4, ItemKind::Open)] };
        assert!(page_show(&seasons).is_some_and(|m| m.save.id == show));
    }
}
