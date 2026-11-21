//! Ordering online subtitles for the auto pick: the preferred language
//! first, then a subtitle made for the playing file (by hash or by release
//! group), then hearing-impaired or not as the user prefers, then a partly
//! matching release, popularity and source. A machine translation ranks
//! below every subtitle people made in the same language.

use portal_api::{SubtitleCandidate, SubtitleMatch};

use super::langs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    OpenSubtitles,
    Gestdown,
    Subdl,
    Subsource,
    TvSubtitles,
    SubtitleCat,
}

impl Source {
    pub const ALL: [Source; 6] = [
        Source::OpenSubtitles,
        Source::Gestdown,
        Source::Subdl,
        Source::Subsource,
        Source::TvSubtitles,
        Source::SubtitleCat,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Source::OpenSubtitles => "OpenSubtitles",
            Source::Gestdown => "Gestdown",
            Source::Subdl => "SubDL",
            Source::Subsource => "SubSource",
            Source::TvSubtitles => "TVsubtitles",
            Source::SubtitleCat => "Subtitle Cat",
        }
    }

    /// The part of a candidate id before the colon.
    pub fn prefix(self) -> &'static str {
        match self {
            Source::OpenSubtitles => "opensubtitles",
            Source::Gestdown => "gestdown",
            Source::Subdl => "subdl",
            Source::Subsource => "subsource",
            Source::TvSubtitles => "tvsubtitles",
            Source::SubtitleCat => "subtitlecat",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<Source> {
        Source::ALL
            .into_iter()
            .find(|source| source.prefix() == prefix)
    }

    fn tiebreak(self) -> u32 {
        match self {
            Source::OpenSubtitles => 30,
            Source::Gestdown
            | Source::Subdl
            | Source::Subsource
            | Source::TvSubtitles
            | Source::SubtitleCat => 20,
        }
    }
}

/// One subtitle a source listed.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    pub source: Source,
    /// A code from `langs`.
    pub language: String,
    pub release: String,
    pub hearing_impaired: bool,
    pub downloads: Option<u64>,
    pub rating: Option<f64>,
    /// The source matched it to the playing file's hash.
    pub hash_match: bool,
    /// Season and episode, to pick the file out of a whole-season zip.
    pub episode: Option<(u32, u32)>,
    /// The source did not say it is the asked-for episode; it ranks lower.
    pub episode_unsure: bool,
    /// A machine translation of someone else's subtitle; it ranks lower.
    pub machine_translated: bool,
}

pub struct Prefs<'a> {
    pub languages: &'a [String],
    pub hearing_impaired: bool,
    /// The playing file's name.
    pub release: Option<&'a str>,
}

const HASH: u32 = 3_000;
const GROUP: u32 = 1_500;
/// More than a partial release match and popularity together.
const HEARING: u32 = 700;
const MEDIUM: u32 = 250;
const RESOLUTION: u32 = 60;
const POPULAR_CAP: u32 = 200;
const RATING_CAP: u32 = 100;
const LANGUAGE_STEP: u32 = 10_000;
const FAMILY_PENALTY: u32 = 5_000;
/// Less than a matching file tag, more than the hearing preference.
const UNSURE_PENALTY: u32 = 1_000;
/// More than a release match, the hearing preference and popularity
/// together, less than a step down the language list.
const MACHINE_PENALTY: u32 = 4_000;
const MACHINE_NOTE: &str = " (machine-translated)";

/// Candidates in a wanted language, best first.
pub fn rank(found: &[Found], prefs: &Prefs) -> Vec<SubtitleCandidate> {
    let playing = prefs.release.map(Release::parse);
    let mut out: Vec<SubtitleCandidate> = found
        .iter()
        .filter_map(|item| {
            let (score, matched) = score(item, prefs, playing.as_ref())?;
            Some(SubtitleCandidate {
                id: item.id.clone(),
                source: item.source.name().to_string(),
                language: item.language.clone(),
                language_name: langs::name_of(&item.language),
                release: shown_release(item),
                hearing_impaired: item.hearing_impaired,
                downloads: item.downloads,
                rating: item.rating,
                score,
                matched,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let rating = |item: &SubtitleCandidate| item.rating.unwrap_or(0.0);
        b.score
            .cmp(&a.score)
            .then_with(|| rating(b).total_cmp(&rating(a)))
            .then_with(|| b.downloads.cmp(&a.downloads))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen = std::collections::HashSet::new();
    out.retain(|item| seen.insert(item.id.clone()));
    out
}

fn score(item: &Found, prefs: &Prefs, playing: Option<&Release>) -> Option<(u32, SubtitleMatch)> {
    let count = prefs.languages.len() as u32;
    let language = if let Some(index) = prefs
        .languages
        .iter()
        .position(|code| code == &item.language)
    {
        (count - index as u32) * LANGUAGE_STEP
    } else {
        let index = prefs
            .languages
            .iter()
            .position(|code| langs::same_family(code, &item.language))?;
        ((count - index as u32) * LANGUAGE_STEP).saturating_sub(FAMILY_PENALTY)
    };
    let mut score = language + item.source.tiebreak();
    let mut matched = SubtitleMatch::Title;
    if item.hash_match {
        score += HASH;
        matched = SubtitleMatch::Hash;
    }
    if let Some(playing) = playing {
        // A subtitle may name several releases it fits, one per line.
        let (points, group) = item
            .release
            .lines()
            .map(|line| release_points(playing, &Release::parse(line)))
            .max()
            .unwrap_or_default();
        score += points;
        if group && matched == SubtitleMatch::Title {
            matched = SubtitleMatch::Release;
        }
    }
    if item.hearing_impaired == prefs.hearing_impaired {
        score += HEARING;
    }
    if item.episode_unsure {
        score = score.saturating_sub(UNSURE_PENALTY);
    }
    if item.machine_translated {
        score = score.saturating_sub(MACHINE_PENALTY);
    }
    if let Some(downloads) = item.downloads {
        let popular = ((downloads as f64 + 1.0).log2() * 12.0) as u32;
        score += popular.min(POPULAR_CAP);
    }
    if let Some(rating) = item
        .rating
        .filter(|rating| rating.is_finite() && *rating > 0.0)
    {
        // Every source rates out of 10.
        score += ((rating * 10.0) as u32).min(RATING_CAP);
    }
    Some((score, matched))
}

/// The first of several release names, marked when machine-translated. The
/// mark is added here, not by the source, so it never reads as a release
/// group when ranking.
fn shown_release(item: &Found) -> String {
    let first = item.release.lines().next().unwrap_or("").trim();
    if item.machine_translated {
        format!("{first}{MACHINE_NOTE}").trim().to_string()
    } else {
        first.to_string()
    }
}

/// Points for how well one release name fits the playing file, and whether
/// its group matched.
fn release_points(playing: &Release, theirs: &Release) -> (u32, bool) {
    let group = playing
        .group
        .as_ref()
        .is_some_and(|group| theirs.tokens.contains(group));
    let mut points = if group { GROUP } else { 0 };
    if playing.medium.is_some() && playing.medium == theirs.medium {
        points += MEDIUM;
    }
    if playing.resolution.is_some() && playing.resolution == theirs.resolution {
        points += RESOLUTION;
    }
    (points, group)
}

/// What a file name says: its tokens, trailing tag, medium and
/// resolution.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Release {
    pub tokens: Vec<String>,
    pub group: Option<String>,
    pub medium: Option<&'static str>,
    pub resolution: Option<&'static str>,
}

/// Words that name a codec, medium or quality, never a file's tag.
const NOT_GROUPS: &[&str] = &[
    "x264", "x265", "h264", "h265", "hevc", "avc", "xvid", "divx", "aac", "ac3", "eac3", "dts",
    "ddp", "dd", "atmos", "web", "dl", "webdl", "webrip", "bluray", "bdrip", "brrip", "hdtv",
    "dvdrip", "remux", "hdr", "sdr", "dv", "10bit", "8bit", "proper", "repack", "internal", "mkv",
    "mp4", "avi", "srt", "en", "eng", "english", "sub", "subs", "hi", "sdh", "cc", "the",
];

/// Codec words a file's tag follows.
const CODECS: &[&str] = &[
    "x264", "x265", "h264", "h265", "264", "265", "hevc", "avc", "xvid", "divx", "aac", "ac3",
    "eac3", "dts", "ddp", "dd", "atmos", "10bit",
];

impl Release {
    pub fn parse(name: &str) -> Release {
        let tokens: Vec<String> = name
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
            .collect();
        Release {
            group: group_of(name),
            medium: medium(&tokens),
            resolution: resolution(&tokens),
            tokens,
        }
    }
}

/// Video and subtitle file extensions, dropped before reading a group.
const EXTENSIONS: &[&str] = &[
    ".mkv", ".mp4", ".avi", ".m4v", ".ts", ".webm", ".mov", ".wmv", ".srt", ".ass", ".ssa", ".sub",
    ".vtt",
];

/// The file's tag: one word after the last hyphen once a quality or source
/// word has come ("x264-TAG", "WEB-DL-TAG[site]"), a bracketed tag there
/// ("x264-[TAG]"), or a lone word ("TAG").
/// The hyphen in "Some-Movie.Title.1080p" names no group.
fn group_of(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let name = EXTENSIONS
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map_or(name, |ext| &name[..name.len() - ext.len()]);
    let name = strip_tags(name);
    let (head, tail) = match name.rsplit_once('-') {
        Some(split) => split,
        None if !name.contains(['.', ' ', '_']) => ("", name),
        None => return None,
    };
    let tail = tail.trim();
    let bracketed = tail.starts_with('[');
    if !bracketed {
        let core = tail.split('[').next().unwrap_or("");
        if core.contains(['.', ' ', '_']) {
            return None;
        }
        let head_tokens: Vec<String> = head
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
            .collect();
        let after_quality = medium(&head_tokens).is_some()
            || resolution(&head_tokens).is_some()
            || head_tokens
                .iter()
                .any(|token| CODECS.contains(&token.as_str()));
        if !head.is_empty() && !after_quality {
            return None;
        }
    }
    let word = tail
        .split(|ch: char| !ch.is_alphanumeric())
        .find(|word| !word.is_empty())?
        .to_lowercase();
    let generic = NOT_GROUPS.contains(&word.as_str())
        || word.bytes().all(|byte| byte.is_ascii_digit())
        || resolution(std::slice::from_ref(&word)).is_some()
        || word.chars().count() < 2;
    (!generic).then_some(word)
}

/// `name` without trailing ".eng", ".en", ".HI", ".SDH" or ".forced" tags,
/// which follow the tag in subtitle file names.
fn strip_tags(mut name: &str) -> &str {
    while let Some((head, tag)) = name.rsplit_once('.') {
        let tag = tag.to_ascii_lowercase();
        let language =
            (2..=3).contains(&tag.len()) && tag.bytes().all(|byte| byte.is_ascii_alphabetic());
        if language || matches!(tag.as_str(), "hi" | "sdh" | "cc" | "forced") {
            name = head;
        } else {
            break;
        }
    }
    name
}

fn medium(tokens: &[String]) -> Option<&'static str> {
    let has = |words: &[&str]| tokens.iter().any(|token| words.contains(&token.as_str()));
    if has(&[
        "bluray",
        "bdrip",
        "brrip",
        "bdremux",
        "remux",
        "bd",
        "bdrip1080p",
    ]) {
        Some("bluray")
    } else if has(&[
        "web", "webdl", "webrip",
    ]) {
        Some("web")
    } else if has(&["hdtv", "pdtv", "hdtvrip", "tvrip", "dsr"]) {
        Some("hdtv")
    } else if has(&["dvdrip", "dvd", "dvd5", "dvd9"]) {
        Some("dvd")
    } else {
        None
    }
}

fn resolution(tokens: &[String]) -> Option<&'static str> {
    let has = |word: &str| tokens.iter().any(|token| token == word);
    if has("2160p") || has("4k") || has("uhd") {
        Some("2160p")
    } else if has("1080p") || has("1080i") {
        Some("1080p")
    } else if has("720p") {
        Some("720p")
    } else if has("480p") || has("576p") {
        Some("480p")
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/rank.rs"]
mod tests;
