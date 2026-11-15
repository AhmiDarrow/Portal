//! Small text helpers shared by the sources.

use serde_json::Value;

/// The longest plot kept from any source.
pub const PLOT_LIMIT: usize = 4000;

/// A title reduced to lowercase letters and digits separated by single
/// spaces, so "Someone's Agents of A.B.C." and "someones agents of abc"
/// compare equal. "&" reads as "and".
pub fn norm_title(title: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in title.replace('&', " and ").chars() {
        if ch.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.extend(ch.to_lowercase());
        } else if ch == '\'' || ch == '\u{2019}' || ch == '.' {
            // Apostrophes and dots join: "S.H.I.E.L.D." is "shield".
        } else {
            gap = true;
        }
    }
    out
}

/// Plain text from a small HTML fragment, as TVmaze sends summaries.
pub fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[open + 1..open + close].trim().to_ascii_lowercase();
        let name = tag.trim_start_matches('/');
        let paragraph = name == "p" || name.starts_with("p ");
        if paragraph || name.starts_with("br") {
            out.push('\n');
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    let decoded = decode_entities(&out);
    let lines: Vec<String> = decoded
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect();
    lines.join("\n")
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|semi| *semi <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` cut to `limit` characters, with an ellipsis when cut.
pub fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push('…');
    cut
}

/// A trimmed, non-empty string at `key`.
pub fn str_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// A plot: trimmed, non-empty, and clipped.
pub fn plot_at(value: &Value, key: &str) -> Option<String> {
    str_at(value, key).map(|plot| clip(&plot, PLOT_LIMIT))
}

/// An id that is a string or number of digits.
pub fn digits_at(value: &Value, key: &str) -> Option<String> {
    let id = match value.get(key)? {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.as_u64()?.to_string(),
        _ => return None,
    };
    portal_engine::kodi_info::all_digits(&id).then_some(id)
}

/// Names from `[{"name": ...}]`, without repeats.
pub fn names_at(value: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for row in value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = row
            .as_str()
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| str_at(row, "name"));
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// Whether `found` (a year from a source) is close enough to the year asked for.
pub fn year_close(asked: Option<u32>, found: Option<u32>) -> bool {
    match (asked, found) {
        (Some(asked), Some(found)) => asked.abs_diff(found) <= 1,
        _ => true,
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/text.rs"]
mod tests;
