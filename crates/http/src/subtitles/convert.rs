//! A downloaded subtitle, whatever its wrapping, as clean WebVTT: the right
//! file out of a zip, its text decoded, and SRT, ASS/SSA or WebVTT cues
//! rewritten with plain text (italic, bold and underline kept), no styling,
//! and a line placed at the top (`{\an8}`) kept at the top.

use std::io::Read;

use encoding_rs::Encoding;

/// The largest subtitle file Portal reads out of an archive.
const MEMBER_CAP: u64 = 6 << 20;
/// The most cues one file may have.
const CUE_CAP: usize = 20_000;

/// Why a download could not become WebVTT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unusable {
    /// A zip without a text subtitle in it, or one Portal cannot open.
    Archive,
    /// No cue could be read.
    Empty,
}

/// WebVTT from a download. `episode` picks the right file out of a season
/// pack; `language` guides the guess at an old file's encoding.
pub fn to_vtt(
    bytes: &[u8],
    episode: Option<(u32, u32)>,
    language: Option<&str>,
) -> Result<String, Unusable> {
    let (bytes, name) = if is_zip(bytes) {
        let (name, member) = unzip(bytes, episode)?;
        (member, Some(name))
    } else {
        (bytes.to_vec(), None)
    };
    let text = decode(&bytes, language);
    let ass = name
        .as_deref()
        .is_some_and(|name| ends_with_any(name, &[".ass", ".ssa"]))
        || looks_like_ass(&text);
    let cues = if ass {
        ass_cues(&text)
    } else if looks_like_microdvd(&text) {
        microdvd_cues(&text)
    } else {
        timed_cues(&text)
    };
    if cues.is_empty() {
        return Err(Unusable::Empty);
    }
    Ok(write_vtt(cues))
}

fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

fn ends_with_any(name: &str, suffixes: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    suffixes.iter().any(|suffix| lower.ends_with(suffix))
}

const SUBTITLE_SUFFIXES: &[&str] = &[".srt", ".vtt", ".ass", ".ssa", ".sub"];

/// The subtitle file to use out of a zip: for an episode, the one named for
/// it (S01E02, 1x02, "Episode 02"), or the only file named for no episode;
/// otherwise SRT before VTT before ASS before MicroDVD, then the largest.
/// A `.sub` next to an `.idx` is a VobSub picture file, not text.
fn unzip(bytes: &[u8], episode: Option<(u32, u32)>) -> Result<(String, Vec<u8>), Unusable> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| Unusable::Archive)?;
    let mut names: Vec<(String, u64)> = Vec::new();
    let mut indexes: Vec<String> = Vec::new();
    for index in 0..archive.len().min(500) {
        let Ok(file) = archive.by_index(index) else {
            continue;
        };
        if file.is_dir() || file.size() > MEMBER_CAP {
            continue;
        }
        let name = file.name().to_string();
        if ends_with_any(&name, &[".idx"]) {
            indexes.push(name[..name.len() - 4].to_ascii_lowercase());
        } else if ends_with_any(&name, SUBTITLE_SUFFIXES) && !name.contains("__MACOSX") {
            names.push((name, file.size()));
        }
    }
    names.retain(|(name, _)| {
        !ends_with_any(name, &[".sub"])
            || !indexes.contains(&name[..name.len() - 4].to_ascii_lowercase())
    });
    let chosen = choose_member(&names, episode).ok_or(Unusable::Archive)?;
    let mut file = archive.by_name(&chosen).map_err(|_| Unusable::Archive)?;
    let mut out = Vec::new();
    file.by_ref()
        .take(MEMBER_CAP)
        .read_to_end(&mut out)
        .map_err(|_| Unusable::Archive)?;
    Ok((chosen, out))
}

pub fn choose_member(names: &[(String, u64)], episode: Option<(u32, u32)>) -> Option<String> {
    let rank = |name: &str| {
        let lower = name.to_ascii_lowercase();
        SUBTITLE_SUFFIXES
            .iter()
            .position(|suffix| lower.ends_with(suffix))
            .unwrap_or(SUBTITLE_SUFFIXES.len())
    };
    let best = |pool: Vec<&(String, u64)>| {
        pool.into_iter()
            .min_by(|a, b| {
                let by_kind = rank(&a.0).cmp(&rank(&b.0));
                by_kind.then(b.1.cmp(&a.1))
            })
            .map(|(name, _)| name.clone())
    };
    if let Some((season, number)) = episode {
        let tagged: Vec<_> = names
            .iter()
            .map(|entry| (entry, member_episode(&entry.0, season)))
            .collect();
        let named: Vec<&(String, u64)> = tagged
            .iter()
            .filter(|(_, found)| *found == Some((season, number)))
            .map(|(entry, _)| *entry)
            .collect();
        if !named.is_empty() {
            return best(named);
        }
        // A file named for another episode is not this one's, and of
        // several files named for none, Portal does not guess.
        let untagged: Vec<&(String, u64)> = tagged
            .iter()
            .filter(|(_, found)| found.is_none())
            .map(|(entry, _)| *entry)
            .collect();
        return match untagged.as_slice() {
            [only] => Some(only.0.clone()),
            _ => None,
        };
    }
    best(names.iter().collect())
}

/// The episode a zip member is named for: "S01E02" or "1x02" in its file
/// name, else in a folder's ("Show.S01E02/eng.srt"), else "Episode 02",
/// "Ep02" or "E02" in its file name, in the asked-for season.
fn member_episode(name: &str, season: u32) -> Option<(u32, u32)> {
    let parts: Vec<&str> = name
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    parts
        .iter()
        .rev()
        .find_map(|part| episode_in_name(part))
        .or_else(|| {
            let number = episode_number(parts.last()?)?;
            Some((season, number))
        })
}

/// "Episode 02", "Ep.02", "Ep02" or "E02" in a name.
fn episode_number(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let number = |text: &str| {
        let plain = (1..=3).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
        plain.then(|| text.parse().ok()).flatten()
    };
    for (index, word) in words.iter().enumerate() {
        for prefix in ["episode", "ep", "e"] {
            let Some(rest) = word.strip_prefix(prefix) else {
                continue;
            };
            if let Some(found) = number(rest) {
                return Some(found);
            }
            if rest.is_empty() && prefix != "e" {
                if let Some(found) = words.get(index + 1).and_then(|next| number(next)) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// The first "S01E02" or "1x02" in a file name.
pub fn episode_in_name(name: &str) -> Option<(u32, u32)> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let digits_at = |start: usize| -> (u32, usize) {
        let mut value = 0u32;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() && end - start < 4 {
            value = value * 10 + u32::from(bytes[end] - b'0');
            end += 1;
        }
        (value, end)
    };
    for index in 0..bytes.len() {
        let before_ok = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
        if !before_ok {
            continue;
        }
        if bytes[index] == b's' {
            let (season, after) = digits_at(index + 1);
            if after > index + 1 && after < bytes.len() && bytes[after] == b'e' {
                let (number, end) = digits_at(after + 1);
                if end > after + 1 {
                    return Some((season, number));
                }
            }
        } else if bytes[index].is_ascii_digit() {
            let (season, after) = digits_at(index);
            if after - index <= 2 && after < bytes.len() && bytes[after] == b'x' {
                let (number, end) = digits_at(after + 1);
                let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
                if end > after + 1 && after_ok {
                    return Some((season, number));
                }
            }
        }
    }
    None
}

/// Text from bytes: a UTF-8 or UTF-16 byte order mark wins, then valid
/// UTF-16 without a mark (many zero bytes), then UTF-8 (with a stray bad
/// byte replaced), then the legacy code page usual for the subtitle's
/// language.
pub fn decode(bytes: &[u8], language: Option<&str>) -> String {
    if let Some((encoding, skip)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[skip..]);
        return text.into_owned();
    }
    // Before UTF-8: zero bytes are valid UTF-8 too.
    if let Some(encoding) = utf16_without_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(bytes);
        return text.into_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    if mostly_utf8(bytes) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let encoding = super::langs::legacy_encoding(language);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// At least 99% of the non-ASCII bytes form valid UTF-8 sequences: a UTF-8
/// file with a stray bad byte, not a legacy one.
fn mostly_utf8(bytes: &[u8]) -> bool {
    let mut good = 0usize;
    let mut bad = 0usize;
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                good += text.bytes().filter(|byte| !byte.is_ascii()).count();
                break;
            }
            Err(err) => {
                let valid = err.valid_up_to();
                good += rest[..valid].iter().filter(|byte| !byte.is_ascii()).count();
                let skip = err.error_len().unwrap_or(rest.len() - valid);
                bad += skip;
                rest = &rest[valid + skip..];
            }
        }
    }
    good > 0 && bad * 100 <= good + bad
}

fn utf16_without_bom(bytes: &[u8]) -> Option<&'static Encoding> {
    if bytes.len() < 8 {
        return None;
    }
    let sample = &bytes[..bytes.len().min(4096) & !1];
    let pairs = sample.len() / 2;
    let even_zeros = sample.iter().step_by(2).filter(|byte| **byte == 0).count();
    let odd_zeros = sample
        .iter()
        .skip(1)
        .step_by(2)
        .filter(|byte| **byte == 0)
        .count();
    if odd_zeros * 10 > pairs * 6 && even_zeros * 10 < pairs {
        Some(encoding_rs::UTF_16LE)
    } else if even_zeros * 10 > pairs * 6 && odd_zeros * 10 < pairs {
        Some(encoding_rs::UTF_16BE)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Shown at the top of the picture rather than the bottom.
    pub top: bool,
}

fn looks_like_ass(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(4000)
        .collect::<String>()
        .to_ascii_lowercase();
    head.contains("[script info]") || head.contains("[events]") || head.contains("\ndialogue:")
}

/// Cues from SRT or WebVTT: a timing line, then text up to a blank line.
/// Cue numbers, identifiers, notes and style blocks fall away.
pub fn timed_cues(text: &str) -> Vec<Cue> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let Some((start, end)) = timing(line) else {
            continue;
        };
        let mut body: Vec<String> = Vec::new();
        while let Some(next) = lines.peek() {
            if next.trim().is_empty() || timing(next).is_some() {
                break;
            }
            body.push(lines.next().unwrap_or_default().to_string());
        }
        // A cue number right before the next timing line is not text.
        if matches!(lines.peek(), Some(next) if timing(next).is_some())
            && body.last().is_some_and(|last| is_number(last))
        {
            body.pop();
        }
        let top = body.iter().find_map(|line| line_top(line)).unwrap_or(false);
        let text = clean_lines(body.iter().map(|line| markup_to_vtt(line)));
        cues.extend(new_cue(start, end, text, top));
        if cues.len() >= CUE_CAP {
            break;
        }
    }
    finish(cues)
}

fn is_number(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit())
}

/// "00:01:02,500 --> 00:01:04,000 X1:..." or WebVTT with cue settings.
fn timing(line: &str) -> Option<(f64, f64)> {
    let (left, right) = line.split_once("-->")?;
    let start = timestamp(left.trim())?;
    let end = timestamp(right.split_whitespace().next()?)?;
    Some((start, end))
}

/// "01:02:03,456", "01:02:03.456", "02:03.456", "1:02:03.4".
pub fn timestamp(text: &str) -> Option<f64> {
    let text = text.trim();
    let (clock, fraction) = match text.rsplit_once([',', '.']) {
        Some((clock, fraction)) if fraction.bytes().all(|byte| byte.is_ascii_digit()) => {
            (clock, fraction)
        }
        _ => (text, ""),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    let number = |part: &str| -> Option<f64> {
        let part = part.trim();
        if part.is_empty() || part.len() > 4 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        part.parse::<f64>().ok()
    };
    let (hours, minutes, seconds) = match parts.as_slice() {
        [hours, minutes, seconds] => (number(hours)?, number(minutes)?, number(seconds)?),
        [minutes, seconds] => (0.0, number(minutes)?, number(seconds)?),
        _ => return None,
    };
    let mut millis = String::from(fraction);
    millis.truncate(3);
    while millis.len() < 3 {
        millis.push('0');
    }
    let millis: f64 = millis.parse().unwrap_or(0.0);
    Some(hours * 3600.0 + minutes * 60.0 + seconds + millis / 1000.0)
}

/// Cues from an ASS/SSA script's `[Events]`, using its `Format:` lines.
/// Signs, songs and drawings fall away, and a line drawn twice (once per
/// layer) shows once.
pub fn ass_cues(text: &str) -> Vec<Cue> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut in_events = false;
    // `Some(legacy)` in a styles section; SSA's `[V4 Styles]` numbers
    // alignments its own way.
    let mut in_styles: Option<bool> = None;
    let mut fields: Vec<String> = [
        "layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let mut style_fields: Vec<String> = Vec::new();
    let mut styles: std::collections::HashMap<String, AssStyle> = Default::default();
    let mut events: Vec<(Cue, bool)> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_start();
        let lower = line.to_ascii_lowercase();
        if line.starts_with('[') {
            in_events = lower.starts_with("[events]");
            in_styles = if lower.starts_with("[v4+ styles]") || lower.starts_with("[v4 styles+]") {
                Some(false)
            } else if lower.starts_with("[v4 styles]") {
                Some(true)
            } else {
                None
            };
            continue;
        }
        if let Some(legacy) = in_styles {
            if let Some(format) = lower.strip_prefix("format:") {
                style_fields = format
                    .split(',')
                    .map(|field| field.trim().to_string())
                    .collect();
            } else if let Some(rest) = line.get(6..).filter(|_| lower.starts_with("style:")) {
                let values: Vec<&str> = rest.splitn(style_fields.len().max(1), ',').collect();
                let at = |name: &str| {
                    style_fields
                        .iter()
                        .position(|field| field == name)
                        .and_then(|index| values.get(index))
                        .map(|value| value.trim())
                };
                let flag = |name: &str| {
                    at(name)
                        .and_then(|value| value.parse::<i32>().ok())
                        .is_some_and(|value| value != 0)
                };
                let alignment = at("alignment").and_then(|value| value.parse::<u32>().ok());
                let style = AssStyle {
                    bold: flag("bold"),
                    italic: flag("italic"),
                    top: alignment.is_some_and(|value| is_top_alignment(value, legacy)),
                };
                if let Some(name) = at("name") {
                    styles.insert(style_key(name), style);
                }
            }
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(format) = lower.strip_prefix("format:") {
            fields = format
                .split(',')
                .map(|field| field.trim().to_string())
                .collect();
            continue;
        }
        let Some(rest) = line.get(9..).filter(|_| lower.starts_with("dialogue:")) else {
            continue;
        };
        let values: Vec<&str> = rest.splitn(fields.len().max(1), ',').collect();
        let at = |name: &str| {
            fields
                .iter()
                .position(|field| field == name)
                .and_then(|index| values.get(index))
                .map(|value| value.trim())
        };
        let (Some(start), Some(end)) = (
            at("start").and_then(timestamp),
            at("end").and_then(timestamp),
        ) else {
            continue;
        };
        let style_name = at("style").unwrap_or("");
        if is_sign_style(style_name) {
            continue;
        }
        let style = styles
            .get(&style_key(style_name))
            .copied()
            .unwrap_or_default();
        // Text is the last field and may hold commas.
        let Some(raw) = fields
            .iter()
            .position(|field| field == "text")
            .and_then(|index| values.get(index))
        else {
            continue;
        };
        let Some(line) = ass_text(raw, style) else {
            continue;
        };
        if let Some(cue) = new_cue(start, end, line.text, line.top) {
            events.push((cue, line.positioned));
        }
        if events.len() >= CUE_CAP {
            break;
        }
    }
    // Signs are placed with `\pos` or `\move`. A script that places most of
    // its lines places its dialogue that way, so then they stay.
    let positioned = events.iter().filter(|(_, positioned)| *positioned).count();
    let keep_positioned = positioned * 2 > events.len();
    let cues = events
        .into_iter()
        .filter(|(_, positioned)| keep_positioned || !positioned)
        .map(|(cue, _)| cue)
        .collect();
    finish(cues)
}

/// What an ASS style gives its lines before any override.
#[derive(Debug, Clone, Copy, Default)]
struct AssStyle {
    bold: bool,
    italic: bool,
    top: bool,
}

fn style_key(name: &str) -> String {
    name.trim().trim_start_matches('*').to_ascii_lowercase()
}

/// A style for signs, typesetting, songs or karaoke rather than dialogue:
/// "Sign", "TypesetSigns", "OP Romaji", "ED-Eng", "Song", "Karaoke".
/// "Default", "Main", "Italics" and "Top" are dialogue.
fn is_sign_style(name: &str) -> bool {
    let lower = style_key(name);
    let word: String = lower
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    lower.contains("sign")
        || lower.contains("typeset")
        || lower.starts_with("song")
        || lower.starts_with("karaoke")
        || matches!(word.as_str(), "op" | "ed" | "opening" | "ending" | "kara")
}

/// An alignment at the top of the picture: 7 to 9 on the numpad, or SSA's
/// 5 to 7.
fn is_top_alignment(value: u32, legacy: bool) -> bool {
    if legacy {
        (5..=7).contains(&value)
    } else {
        (7..=9).contains(&value)
    }
}

/// One override tag in a `{\…}` block, without its backslash.
enum Override {
    /// `\i1`, `\i0`; `\i` alone goes back to the style's.
    Italic(Option<bool>),
    Bold(Option<bool>),
    /// `\an8` or `\a6`: whether the line sits at the top.
    Top(bool),
    /// `\pos(…)` or `\move(…)`.
    Positioned,
    /// `\p1` starts a drawing, `\p0` ends it.
    Drawing(bool),
    /// `\r`: back to the style.
    Reset,
    Other,
}

fn override_tag(tag: &str) -> Override {
    let number = |rest: &str| {
        let plain = !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit());
        plain.then(|| rest.parse::<u32>().ok()).flatten()
    };
    if tag.starts_with("pos(") || tag.starts_with("move(") {
        return Override::Positioned;
    }
    if let Some(value) = tag.strip_prefix("an").and_then(number) {
        return Override::Top(is_top_alignment(value, false));
    }
    if let Some(value) = tag.strip_prefix('a').and_then(number) {
        return Override::Top(is_top_alignment(value, true));
    }
    if let Some(value) = tag.strip_prefix('p').and_then(number) {
        return Override::Drawing(value > 0);
    }
    if tag == "i" {
        return Override::Italic(None);
    }
    if let Some(value) = tag.strip_prefix('i').and_then(number) {
        return Override::Italic(Some(value != 0));
    }
    if tag == "b" {
        return Override::Bold(None);
    }
    if let Some(value) = tag.strip_prefix('b').and_then(number) {
        // `\b1`, or a weight such as `\b700`.
        return Override::Bold(Some(value == 1 || value >= 600));
    }
    if tag.starts_with('r') {
        return Override::Reset;
    }
    Override::Other
}

/// Whether the first `{\an8}`-style alignment in a line puts it at the top.
fn line_top(line: &str) -> Option<bool> {
    let mut rest = line;
    while let Some(open) = rest.find("{\\") {
        let block = &rest[open + 1..];
        let close = block.find('}')?;
        for tag in block[..close].split('\\') {
            if let Override::Top(top) = override_tag(tag.trim()) {
                return Some(top);
            }
        }
        rest = &block[close + 1..];
    }
    None
}

/// A dialogue line as cue text.
struct AssLine {
    text: String,
    top: bool,
    positioned: bool,
}

/// Dialogue text without override blocks; `\N` breaks lines and `\i1`,
/// `\b1` become `<i>`, `<b>`. Drawings (`\p1`) are not text and come back
/// as `None`.
fn ass_text(raw: &str, style: AssStyle) -> Option<AssLine> {
    let mut out = String::new();
    let mut rest = raw;
    // (bold, italic) as wanted and as written so far.
    let mut want = (style.bold, style.italic);
    let mut shown = (false, false);
    let mut top = None;
    let mut positioned = false;
    let mut drawing = false;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('{') {
            let Some(close) = stripped.find('}') else {
                break;
            };
            for tag in stripped[..close].split('\\') {
                match override_tag(tag.trim()) {
                    Override::Italic(on) => want.1 = on.unwrap_or(style.italic),
                    Override::Bold(on) => want.0 = on.unwrap_or(style.bold),
                    // The first alignment in a line is the one shown.
                    Override::Top(at_top) => {
                        top.get_or_insert(at_top);
                    }
                    Override::Positioned => positioned = true,
                    Override::Drawing(on) => drawing |= on,
                    Override::Reset => want = (style.bold, style.italic),
                    Override::Other => {}
                }
            }
            rest = &stripped[close + 1..];
            continue;
        }
        let next = rest.find(['{', '\\']).unwrap_or(rest.len());
        let (chunk, after) = if next == 0 {
            // A backslash escape: \N, \n, \h.
            let width = 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
            (&rest[..width], &rest[width..])
        } else {
            (&rest[..next], &rest[next..])
        };
        match chunk {
            "\\N" | "\\n" => {
                // Tags close at each line's end, so a line is whole alone.
                write_tags(&mut out, &mut shown, (false, false));
                out.push('\n');
            }
            _ => {
                write_tags(&mut out, &mut shown, want);
                let chunk = if chunk == "\\h" { " " } else { chunk };
                out.push_str(&escape_text(chunk));
            }
        }
        rest = after;
    }
    write_tags(&mut out, &mut shown, (false, false));
    let text = clean_lines(out.split('\n').map(String::from));
    (!drawing && !text.is_empty()).then(|| AssLine {
        text,
        top: top.unwrap_or(style.top),
        positioned,
    })
}

/// Closes the open `<b>`/`<i>` and opens the wanted ones, so they nest.
fn write_tags(out: &mut String, shown: &mut (bool, bool), want: (bool, bool)) {
    if *shown == want {
        return;
    }
    if shown.1 {
        out.push_str("</i>");
    }
    if shown.0 {
        out.push_str("</b>");
    }
    if want.0 {
        out.push_str("<b>");
    }
    if want.1 {
        out.push_str("<i>");
    }
    *shown = want;
}

/// "{100}{250}Text" on the first line with text: MicroDVD.
fn looks_like_microdvd(text: &str) -> bool {
    text.lines()
        .map(|line| line.trim().trim_start_matches('\u{feff}'))
        .find(|line| !line.is_empty())
        .is_some_and(|line| microdvd_line(line).is_some())
}

/// A MicroDVD line's start and end frames and its text.
fn microdvd_line(line: &str) -> Option<(u64, Option<u64>, &str)> {
    let (start, rest) = line.strip_prefix('{')?.split_once('}')?;
    let (end, text) = rest.strip_prefix('{')?.split_once('}')?;
    let frame = |text: &str| {
        let plain = (1..=9).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
        plain.then(|| text.parse::<u64>().ok()).flatten()
    };
    let end = if end.is_empty() {
        None
    } else {
        Some(frame(end)?)
    };
    Some((frame(start)?, end, text))
}

/// Cues from MicroDVD: frames at 23.976 a second unless the first line
/// gives the rate ("{1}{1}25"); `|` breaks lines, `{y:i}`-style codes and
/// a leading `/` fall away.
pub fn microdvd_cues(text: &str) -> Vec<Cue> {
    let mut fps = 23.976;
    let mut first = true;
    let mut cues = Vec::new();
    for line in text.lines() {
        let Some((start, end, body)) = microdvd_line(line.trim().trim_start_matches('\u{feff}'))
        else {
            continue;
        };
        if std::mem::take(&mut first) && start <= 1 {
            if let Ok(rate) = body.trim().parse::<f64>() {
                if (1.0..=200.0).contains(&rate) {
                    fps = rate;
                    continue;
                }
            }
        }
        let text = clean_lines(body.split('|').map(|part| {
            let mut plain = String::new();
            let mut rest = part;
            while let Some(open) = rest.find('{') {
                plain.push_str(&rest[..open]);
                match rest[open..].find('}') {
                    Some(close) => rest = &rest[open + close + 1..],
                    None => {
                        rest = "";
                    }
                }
            }
            plain.push_str(rest);
            escape_text(plain.trim().trim_start_matches('/'))
        }));
        let end = end.map_or(0.0, |end| end as f64 / fps);
        cues.extend(new_cue(start as f64 / fps, end, text, false));
        if cues.len() >= CUE_CAP {
            break;
        }
    }
    finish(cues)
}

/// SRT or WebVTT cue text as WebVTT: `<i>`, `<b>` and `<u>` kept, every
/// other tag and `{\an8}`-style override dropped (`line_top` reads those),
/// entities such as `&quot;` and `&#39;` read, `&` and `<` escaped.
fn markup_to_vtt(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with('{') {
            if let Some(close) = rest.find('}') {
                if rest[1..close].starts_with('\\') {
                    rest = &rest[close + 1..];
                    continue;
                }
            }
        }
        if rest.starts_with('<') {
            if let Some(close) = rest.find('>') {
                let tag = rest[1..close].trim().to_ascii_lowercase();
                let name = tag.trim_start_matches('/');
                let name = name
                    .split(|ch: char| !ch.is_ascii_alphanumeric())
                    .next()
                    .unwrap_or("");
                if matches!(name, "i" | "b" | "u") {
                    let slash = if tag.starts_with('/') { "/" } else { "" };
                    out.push_str(&format!("<{slash}{name}>"));
                }
                rest = &rest[close + 1..];
                continue;
            }
        }
        if let Some((ch, width)) = entity(rest) {
            out.push_str(&escape_text(&ch.to_string()));
            rest = &rest[width..];
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        out.push_str(&escape_text(&ch.to_string()));
        rest = &rest[ch.len_utf8()..];
    }
    decode_basic_entities(&out)
}

/// The HTML entity at the start of `text` ("&quot;", "&#39;", "&#x2019;")
/// and its length.
fn entity(text: &str) -> Option<(char, usize)> {
    if !text.starts_with('&') {
        return None;
    }
    let semi = text
        .char_indices()
        .take(12)
        .find(|(_, ch)| *ch == ';')
        .map(|(at, _)| at)?;
    let ch = match &text[1..semi] {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        body => {
            let number = body.strip_prefix('#')?;
            let value = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(value).filter(|ch| !ch.is_control())?
        }
    };
    Some((ch, semi + 1))
}

/// WebVTT's own entities stay, but "&amp;amp;" from double escaping does not.
fn decode_basic_entities(text: &str) -> String {
    text.replace("&amp;amp;", "&amp;")
        .replace("&amp;lt;", "&lt;")
        .replace("&amp;gt;", "&gt;")
        .replace("&amp;nbsp;", "&nbsp;")
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Trimmed, non-empty lines joined by newlines. "-->" cannot appear in cue
/// text.
fn clean_lines<I: Iterator<Item = String>>(lines: I) -> String {
    lines
        .map(|line| line.replace("--&gt;", "-&gt;").replace("-->", "->"))
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !is_empty_markup(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_empty_markup(line: &str) -> bool {
    let mut stripped = line.to_string();
    for tag in ["<i>", "</i>", "<b>", "</b>", "<u>", "</u>"] {
        stripped = stripped.replace(tag, "");
    }
    stripped.trim().is_empty()
}

fn new_cue(start: f64, end: f64, text: String, top: bool) -> Option<Cue> {
    if text.is_empty() || !start.is_finite() || !end.is_finite() || start < 0.0 {
        return None;
    }
    let end = if end > start { end } else { start + 2.0 };
    Some(Cue {
        start,
        end,
        text,
        top,
    })
}

/// In time order, a cue repeated with the same times and text once.
fn finish(mut cues: Vec<Cue>) -> Vec<Cue> {
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut seen = std::collections::HashSet::new();
    cues.retain(|cue| seen.insert((cue.start.to_bits(), cue.end.to_bits(), cue.text.clone())));
    cues
}

/// The cue setting that puts a cue at the top of the picture.
const TOP_SETTING: &str = "line:0";

pub fn write_vtt(cues: Vec<Cue>) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        out.push_str(&clock(cue.start));
        out.push_str(" --> ");
        out.push_str(&clock(cue.end));
        if cue.top {
            out.push(' ');
            out.push_str(TOP_SETTING);
        }
        out.push('\n');
        out.push_str(&cue.text);
        out.push_str("\n\n");
    }
    out
}

fn clock(seconds: f64) -> String {
    let total = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        total / 3_600_000,
        (total / 60_000) % 60,
        (total / 1000) % 60,
        total % 1000
    )
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/convert.rs"]
mod tests;
