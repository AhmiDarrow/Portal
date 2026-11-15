//! The season and episode a file name or link states, so a play can tell
//! when an add-on opened a different episode than the one asked for.

use std::collections::BTreeSet;

/// The one season and episode this path names, when it names exactly one.
/// A span such as `S04E01E02`, or a name with no season-episode pair, is none.
pub(crate) fn stated_episode(path: &str) -> Option<(u32, u32)> {
    match scan_pairs(file_name(path)) {
        Scan::One(season, episode) => Some((season, episode)),
        Scan::Many => None,
        Scan::None => stated_from_parent(path),
    }
}

/// The episode named by a play URL or path. Each path segment is percent-decoded
/// on its own, and a `+` stays a plus. A query or a header after `|` is ignored.
pub(crate) fn named_episode(url_or_path: &str) -> Option<(u32, u32)> {
    stated_episode(&decoded_media_path(url_or_path))
}

/// True when the name states one episode and it is not the one asked for.
pub(crate) fn episode_conflict(url_or_path: &str, season: u32, episode: u32) -> bool {
    named_episode(url_or_path).is_some_and(|(found_season, found_episode)| {
        found_season != season || found_episode != episode
    })
}

enum Scan {
    One(u32, u32),
    Many,
    None,
}

fn scan_pairs(filename: &str) -> Scan {
    let text = filename.to_lowercase();
    let bytes = text.as_bytes();
    let mut pairs = BTreeSet::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if !boundary_before(&text, index) {
            index += utf8_len(bytes[index]);
            continue;
        }
        if bytes[index] == b's' {
            if let Some((consumed, season, episodes)) = parse_season_episode(&text[index + 1..]) {
                if episodes.len() != 1 {
                    return Scan::Many;
                }
                pairs.insert((season, episodes[0]));
                index += 1 + consumed;
                continue;
            }
        }
        if bytes[index].is_ascii_digit() {
            if let Some((consumed, season, episode)) = parse_cross(&text[index..]) {
                pairs.insert((season, episode));
                index += consumed;
                continue;
            }
        }
        index += utf8_len(bytes[index]);
    }
    match pairs.len() {
        0 => Scan::None,
        1 => {
            let (season, episode) = pairs.into_iter().next().expect("one pair");
            Scan::One(season, episode)
        }
        _ => Scan::Many,
    }
}

fn stated_from_parent(path: &str) -> Option<(u32, u32)> {
    let parts = components(path);
    if parts.len() < 2 {
        return None;
    }
    let mut seasons = BTreeSet::new();
    for part in &parts[..parts.len() - 1] {
        for season in 1..=99 {
            if component_season(part, season) {
                seasons.insert(season);
            }
        }
    }
    let season = single(&seasons.into_iter().collect::<Vec<_>>())?;
    let episode = single(&bare_numbers(parts.last()?))?;
    Some((season, episode))
}

/// Path without a scheme, host, query, fragment, or Kodi header. Each segment
/// is percent-decoded on its own, so an encoded slash stays inside a name.
fn decoded_media_path(url_or_path: &str) -> String {
    let base = url_or_path.split('|').next().unwrap_or(url_or_path).trim();
    let path = base.split(['?', '#']).next().unwrap_or(base);
    let path = match path.find("://") {
        Some(scheme) => {
            let after = &path[scheme + 3..];
            after.find('/').map(|slash| &after[slash..]).unwrap_or("")
        }
        None => path,
    };
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(percent_decode_file)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_decode_file(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn single(episodes: &[u32]) -> Option<u32> {
    let unique: BTreeSet<u32> = episodes.iter().copied().collect();
    if unique.len() == 1 {
        unique.iter().next().copied()
    } else {
        None
    }
}

fn parse_season_episode(rest: &str) -> Option<(usize, u32, Vec<u32>)> {
    let (season, after_season) = take_digits(rest, 1, 2)?;
    if !(1..=99).contains(&season) {
        return None;
    }
    let mut cursor = after_season;
    if let Some(stripped) = strip_sep(cursor) {
        if stripped.starts_with('e') {
            cursor = stripped;
        }
    }
    let cursor = cursor.strip_prefix('e')?;
    let (first, mut after) = episode_number(cursor)?;
    let mut episodes = vec![first];
    while let Some((episode, next)) = extra_episode(after) {
        episodes.push(episode);
        after = next;
    }
    if !ends_ok(after) {
        return None;
    }
    Some((rest.len() - after.len(), season, episodes))
}

fn parse_cross(text: &str) -> Option<(usize, u32, u32)> {
    let (season, after) = take_digits(text, 1, 2)?;
    if !(1..=99).contains(&season) {
        return None;
    }
    let after = after.strip_prefix('x')?;
    let (episode, after) = episode_number(after)?;
    if !ends_ok(after) {
        return None;
    }
    Some((text.len() - after.len(), season, episode))
}

fn extra_episode(rest: &str) -> Option<(u32, &str)> {
    let mut look = rest;
    if let Some(stripped) = strip_sep(look) {
        if stripped.starts_with('e') {
            look = stripped;
        }
    }
    let look = look.strip_prefix('e')?;
    episode_number(look)
}

fn episode_number(text: &str) -> Option<(u32, &str)> {
    let (episode, after) = take_digits(text, 1, 3)?;
    if !(1..=999).contains(&episode) || after.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    Some((episode, after))
}

fn bare_numbers(filename: &str) -> Vec<u32> {
    let text = filename.to_lowercase();
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if !boundary_before(&text, index) {
            index += utf8_len(bytes[index]);
            continue;
        }
        if let Some((episode, next)) = labeled_number(&text, index, "episode") {
            found.push(episode);
            index = next;
            continue;
        }
        if let Some((episode, next)) = labeled_number(&text, index, "ep") {
            found.push(episode);
            index = next;
            continue;
        }
        if let Some((episode, next)) = e_number(&text, index) {
            found.push(episode);
            index = next;
            continue;
        }
        index += utf8_len(bytes[index]);
    }
    found
}

fn labeled_number(text: &str, index: usize, word: &str) -> Option<(u32, usize)> {
    let rest = text.get(index..)?.strip_prefix(word)?;
    if rest.starts_with(|ch: char| ch.is_ascii_alphanumeric()) {
        return None;
    }
    let mut after = rest;
    while let Some(stripped) = strip_sep(after) {
        after = stripped;
    }
    let (episode, after) = episode_number(after)?;
    if !ends_ok(after) {
        return None;
    }
    Some((episode, text.len() - after.len()))
}

fn e_number(text: &str, index: usize) -> Option<(u32, usize)> {
    let rest = text.get(index..)?.strip_prefix('e')?;
    let (episode, after) = episode_number(rest)?;
    if !ends_ok(after) {
        return None;
    }
    Some((episode, text.len() - after.len()))
}

fn component_season(part: &str, season: u32) -> bool {
    let tokens = tokens(&part.to_lowercase());
    if tokens.windows(2).any(|pair| {
        pair[0] == "season"
            && pair[1]
                .parse::<u32>()
                .ok()
                .is_some_and(|number| number == season)
    }) {
        return true;
    }
    tokens.iter().any(|token| season_tag(token, season))
}

fn season_tag(token: &str, season: u32) -> bool {
    let Some(rest) = token.strip_prefix('s') else {
        return false;
    };
    (1..=2).contains(&rest.len()) && rest.parse::<u32>().ok() == Some(season)
}

fn file_name(path: &str) -> &str {
    components(path).last().copied().unwrap_or(path)
}

fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect()
}

fn boundary_before(text: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    text[..index]
        .chars()
        .next_back()
        .is_some_and(|ch| !ch.is_ascii_alphanumeric())
}

fn ends_ok(rest: &str) -> bool {
    rest.chars()
        .next()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric())
}

fn strip_sep(text: &str) -> Option<&str> {
    match text.chars().next() {
        Some('.' | '_' | '-' | ' ') => Some(&text[1..]),
        _ => None,
    }
}

fn take_digits(text: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let count = text.bytes().take_while(u8::is_ascii_digit).count();
    if count < min {
        return None;
    }
    let (number, rest) = text.split_at(count.min(max));
    Some((number.parse().ok()?, rest))
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

fn tokens(lower: &str) -> Vec<String> {
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/episode_name.rs"]
mod tests;
