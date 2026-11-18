//! Picks a copy from an add-on's list of sources for the quality asked for.
//!
//! An add-on's source list can name many copies, best first by its own
//! measure, which often puts the largest 4K files at the top. A 1080p play
//! should not pull a 4K file only to shrink it, so Portal reads each row's
//! resolution and takes the best copy at or under the height asked for.
//! Among copies of that height, one the browser can play as it is (SDR
//! H.264, not a full-disc copy) wins; otherwise the add-on's order stands.
//!
//! For a show, a whole-season file that fits the height is the default,
//! ahead of a single episode file. The season file the viewer already played
//! still wins over others for later episodes of that season, when a row
//! still names it; its title is the same on every episode's list. A single
//! episode file names one episode, so it is not a season file. This follows
//! the row's title, for any show and any add-on. A movie row has no season,
//! so it is unchanged.

/// A copy's resolution as its row names it, such as `1080p`, `2160p` or `4K`.
/// None when the row does not say.
pub fn row_height(row: &str) -> Option<u32> {
    let lower = row.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    // A number such as 1080p is the clearest; "4K" or "UHD" only says 2160.
    for word in &words {
        let digits = word
            .strip_suffix('p')
            .or_else(|| word.strip_suffix('i'))
            .unwrap_or("");
        if let Ok(height) = digits.parse::<u32>() {
            if matches!(height, 2160 | 1440 | 1080 | 720 | 576 | 540 | 480 | 360) {
                return Some(height);
            }
        }
    }
    if words
        .iter()
        .any(|word| matches!(*word, "4k" | "uhd" | "2160"))
    {
        return Some(2160);
    }
    if words.contains(&"sd") {
        return Some(480);
    }
    None
}

/// How much work a copy of the right height still is: HDR needs tone
/// mapping, HEVC, AV1 and VC-1 need encoding, and a remux is several times
/// the bitrate. Lower is better.
fn burden(row: &str) -> u32 {
    let lower = row.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let has = |names: &[&str]| words.iter().any(|word| names.contains(word));
    let mut cost = 0;
    if has(&["hdr", "hdr10", "dv", "dovi", "hlg"])
        || lower.contains("dolby.vision")
        || lower.contains("dolby vision")
    {
        cost += 2;
    }
    if has(&["hevc", "x265", "h265", "av1", "vc1", "mpeg2"])
        || lower.contains("h.265")
        || lower.contains("vc-1")
    {
        cost += 1;
    }
    if has(&["remux", "bdremux"]) {
        cost += 1;
    }
    cost
}

/// The row to play for a picture `cap` pixels tall, leaving out rows already
/// tried. `preferred` is a season file's identity from an earlier episode.
/// None when no row names a resolution, so the list is not a source list, or
/// when every row has been tried.
pub fn pick_source(
    rows: &[String],
    cap: u32,
    tried: &[String],
    preferred: Option<&str>,
) -> Option<usize> {
    if !rows.iter().any(|row| row_height(row).is_some()) {
        return None;
    }
    let preferred = preferred.filter(|id| !id.is_empty());
    rows.iter()
        .enumerate()
        .filter(|(_, row)| !tried.contains(row))
        .min_by_key(|(index, row)| {
            // A season file already chosen wins when it fits. Then rows
            // that fit, and among those a whole-season file before a single
            // episode, closest height first. Rows that do not say come next.
            // Copies too tall are last, smallest first, and a season file does not
            // jump the quality cap.
            let saved = pack_rank(row, cap, preferred);
            let (fit, distance) = match row_height(row) {
                Some(height) if height <= cap => (0u8, cap - height),
                None => (1, 0),
                Some(height) => (2, height - cap),
            };
            let single = u8::from(fit == 2 || pack_identity(row).is_none());
            (saved, fit, single, distance, burden(row), *index)
        })
        .map(|(index, _)| index)
}

/// Identity of a whole-season or whole-show file, for matching the same file on the
/// next episode. None when the row is one episode file or does not name a
/// release. Add-ons do not have to write the word "pack": a title that names
/// a season, a range, or a complete series is enough.
pub fn pack_identity(row: &str) -> Option<String> {
    let key = release_key(row)?;
    // One episode number is a single file. A range names two or more.
    if episode_count(&key) == 1 {
        return None;
    }
    Some(key)
}

/// How well `row` matches the remembered season file: 0 when it is that
/// file, 1 when the row names that file's whole release among other words,
/// 2 when it is not that file. The second keeps the pack when an add-on
/// changes how it lays out a row, as an update can, adding a size or a tag
/// next to the name. A row taller than `cap` never matches; one that does
/// not name a height still can, and a 4K file does not stick for a 1080p play.
fn pack_rank(row: &str, cap: u32, preferred: Option<&str>) -> u8 {
    let Some(preferred) = preferred.filter(|id| !id.is_empty()) else {
        return 2;
    };
    if matches!(row_height(row), Some(height) if height > cap) {
        return 2;
    }
    if pack_identity(row).as_deref() == Some(preferred) {
        return 0;
    }
    // Whole words only, so a remembered name never matches inside a longer word.
    let words = format!(".{}.", dotted(row));
    if words.contains(&format!(".{preferred}.")) {
        return 1;
    }
    2
}

/// The title on the row: the name before the dash, or the longest
/// other part that names a season. Episode numbers stay, so a single file
/// does not compare equal to the season file it was cut from.
fn release_key(row: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for part in row.split(['\u{2014}', '|']) {
        let Some(key) = normalize_release(part) else {
            continue;
        };
        let replace = match &best {
            None => true,
            Some(current) => key.len() > current.len(),
        };
        if replace {
            best = Some(key);
        }
    }
    best
}

fn normalize_release(part: &str) -> Option<String> {
    let key = dotted(part);
    if key.is_empty() {
        return None;
    }
    let stripped = strip_leading_index(&key);
    if looks_like_release(stripped) {
        Some(stripped.to_string())
    } else if stripped != key && looks_like_release(&key) {
        Some(key)
    } else {
        None
    }
}

fn dotted(text: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('.');
            }
            gap = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    out
}

/// A leading `01.` is a list index. A show whose name is a number stays,
/// because the next word is the season.
fn strip_leading_index(key: &str) -> &str {
    let Some((first, rest)) = key.split_once('.') else {
        return key;
    };
    if rest.is_empty()
        || first.len() > 2
        || !first.chars().all(|ch| ch.is_ascii_digit())
        || season_word(rest.split('.').next().unwrap_or(""))
    {
        return key;
    }
    rest
}

fn looks_like_release(key: &str) -> bool {
    let alnum = key.chars().filter(|ch| ch.is_ascii_alphanumeric()).count();
    if alnum < 8 || !key.chars().any(|ch| ch.is_ascii_digit()) {
        return false;
    }
    key.split('.')
        .any(|word| word == "complete" || word == "season" || season_word(word))
}

fn season_word(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('s') else {
        return false;
    };
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if !(1..=2).contains(&digits) {
        return false;
    }
    let after = &rest[digits..];
    after.is_empty() || episode_count_word(after) > 0
}

/// How many episode numbers the release names. Zero is a season or a
/// complete series. One is a single file. Two or more is a range.
fn episode_count(key: &str) -> usize {
    let words: Vec<&str> = key.split('.').filter(|word| !word.is_empty()).collect();
    let mut count = 0;
    let mut index = 0;
    while index < words.len() {
        if matches!(words[index], "episode" | "ep")
            && words
                .get(index + 1)
                .is_some_and(|word| !word.is_empty() && word.chars().all(|ch| ch.is_ascii_digit()))
        {
            count += 1;
            index += 2;
            continue;
        }
        count += episode_count_word(words[index]);
        index += 1;
    }
    count
}

fn episode_count_word(word: &str) -> usize {
    if episode_cross(word) {
        return 1;
    }
    let mut rest = word;
    if let Some(after) = word.strip_prefix('s') {
        let digits = after.chars().take_while(|ch| ch.is_ascii_digit()).count();
        if (1..=2).contains(&digits) {
            rest = &after[digits..];
        }
    }
    let bytes = rest.as_bytes();
    let mut count = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'e' && index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
            count += 1;
            index += 2;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    count
}

/// `3x06` and `12x7`: a season and episode written with an x.
fn episode_cross(word: &str) -> bool {
    let bytes = word.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == 0 || index > 2 || index >= bytes.len() || bytes[index] != b'x' {
        return false;
    }
    let rest = &word[index + 1..];
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    (1..=3).contains(&digits) && digits == rest.len()
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/sources.rs"]
mod tests;
