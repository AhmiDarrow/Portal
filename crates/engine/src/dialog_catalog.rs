//! Discover add-on skin dialog XMLs so Portal can mirror any modal an action opens.
//! No add-on names are hard-coded: roots are scanned and Window.Property names are
//! taken from each XML.

use portal_api::PromptKind;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(60);
/// More windows than this in one Kodi query are not checked.
const MAX_OVERLAYS: usize = 64;

/// `Window.IsActive(file)` for every installed add-on window shaped like a
/// busy overlay. Refreshed with the catalog, so playback can ask
/// Kodi about them without a catalog of its own.
static OVERLAYS: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// The overlay windows found in installed add-ons, as Kodi info booleans.
pub(crate) fn overlay_keys() -> Vec<String> {
    OVERLAYS
        .read()
        .map(|keys| keys.clone())
        .unwrap_or_default()
}

/// A progress window with no list: the add-on is working and will close it
/// itself. A window with a list is something to pick from.
fn is_overlay(dialog: &SkinDialog) -> bool {
    dialog.kind == PromptKind::Progress && dialog.list.is_none()
}

/// Skin dialog Portal may surface when `Window.IsActive(file)` is true.
#[derive(Debug, Clone)]
pub struct SkinDialog {
    pub file: String,
    pub kind: PromptKind,
    pub properties: Vec<String>,
    pub control_ids: Vec<i64>,
    /// Label, textbox, and fadelabel ids. Their `Control.GetLabel` text is the
    /// dialog body. Progress and image controls stay out of this list.
    pub text_ids: Vec<i64>,
    /// The add-on's own window rather than a known dialog shape. Its Ok
    /// closes it, since Portal cannot know what its focused button does.
    pub custom: bool,
    /// The first list or panel in the window, when it has one.
    pub list: Option<SkinList>,
    /// Button controls outside list layouts, in document order. A window
    /// whose list is empty can still offer its buttons.
    pub buttons: Vec<i64>,
}

/// A list or panel in a skin dialog, read item by item through its container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinList {
    pub id: i64,
    pub horizontal: bool,
    /// ListItem info names the item layout shows, such as `Label` or
    /// `Property(name)`, the item's name first.
    pub fields: Vec<String>,
}

#[derive(Debug, Default)]
pub struct DialogCatalog {
    roots: Vec<PathBuf>,
    dialogs: Vec<SkinDialog>,
    loaded_at: Option<Instant>,
}

impl DialogCatalog {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let mut catalog = Self {
            roots,
            dialogs: Vec::new(),
            loaded_at: None,
        };
        catalog.refresh();
        catalog
    }

    pub fn dialogs(&mut self) -> &[SkinDialog] {
        let stale = match self.loaded_at {
            None => true,
            Some(at) => at.elapsed() >= REFRESH,
        };
        if stale {
            self.refresh();
        }
        &self.dialogs
    }

    pub fn refresh(&mut self) {
        let mut found = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for root in &self.roots {
            let addons = root.join("addons");
            let base = if addons.is_dir() {
                addons
            } else {
                root.clone()
            };
            scan_addons(&base, &mut found, &mut seen);
        }
        let overlays: Vec<String> = found
            .iter()
            .filter(|dialog| is_overlay(dialog))
            .map(|dialog| format!("Window.IsActive({})", dialog.file))
            .take(MAX_OVERLAYS)
            .collect();
        if let Ok(mut keys) = OVERLAYS.write() {
            *keys = overlays;
        }
        self.dialogs = found;
        self.loaded_at = Some(Instant::now());
    }
}

fn scan_addons(
    addons: &Path,
    out: &mut Vec<SkinDialog>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    let Ok(entries) = fs::read_dir(addons) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skins = path.join("resources").join("skins");
        if !skins.is_dir() {
            continue;
        }
        walk_skins(&skins, out, seen, 0);
    }
}

/// Skin folders are a few levels deep (`Default/1080i`); deeper is not a skin.
const SKIN_DEPTH: usize = 6;
/// A dialog XML is small; anything bigger is not read.
const SKIN_FILE_CAP: u64 = 1024 * 1024;

fn walk_skins(
    dir: &Path,
    out: &mut Vec<SkinDialog>,
    seen: &mut std::collections::BTreeSet<String>,
    depth: usize,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // Not followed: links and junctions can loop or lead out of the add-on.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            if depth < SKIN_DEPTH {
                walk_skins(&path, out, seen, depth + 1);
            }
            continue;
        }
        if entry.metadata().map_or(true, |meta| meta.len() > SKIN_FILE_CAP) {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("xml") {
            continue;
        }
        let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !seen.insert(file.to_ascii_lowercase()) {
            continue;
        }
        if skip_dialog(file) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(dialog) = parse_dialog(file, &text) {
            out.push(dialog);
        }
    }
}

/// Windows a skin or add-on shows over a playing video (play next, up next,
/// skip intro, still watching). They never ask anything before a play, so
/// they are left out whatever the skin prefixes their file name with.
fn skip_dialog(file: &str) -> bool {
    let plain: String = file
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    ["playnext", "upnext", "skipintro", "stillwatching"]
        .iter()
        .any(|word| plain.contains(word))
}

fn parse_dialog(file: &str, text: &str) -> Option<SkinDialog> {
    let properties = property_names(text);
    let control_ids = control_ids(text);
    let text_ids = text_control_ids(text);
    let mut kind = classify_skin(file, text, &properties);
    let file_lower = file.to_ascii_lowercase();
    let list = list_control(text);
    let buttons = button_ids(text);
    let custom = matches!(kind, PromptKind::Ok) && !named_ok(&file_lower);
    // A source list or other picker in the add-on's own window.
    if custom && list.is_some() {
        kind = PromptKind::Select;
    }
    // A manager window is still a dialog when its file name does not say so.
    // The list and the buttons are what the viewer has to answer.
    let looks_like_dialog = !properties.is_empty()
        || list.is_some()
        || !buttons.is_empty()
        || text.contains("type=\"progress\"")
        || file.eq_ignore_ascii_case("ok.xml")
        || file.eq_ignore_ascii_case("confirm.xml")
        || file_lower.contains("progress")
        || file_lower.contains("confirm")
        || file_lower.contains("dialog")
        || file_lower.contains("status")
        || file_lower.contains("resolve");
    if !looks_like_dialog {
        return None;
    }
    Some(SkinDialog {
        file: file.to_string(),
        kind,
        properties,
        control_ids,
        text_ids,
        custom,
        list,
        buttons,
    })
}

/// Button ids outside item layouts. List rows are not buttons.
fn button_ids(text: &str) -> Vec<i64> {
    let text = strip_comments(text);
    let text = cut_blocks(&text, "itemlayout");
    let text = cut_blocks(&text, "focusedlayout");
    let mut ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("<control") {
        let start = from + at;
        let Some(close) = text[start..].find('>') else {
            break;
        };
        let tag = &text[start..=start + close];
        from = start + close + 1;
        let Some(kind) = attribute(tag, "type") else {
            continue;
        };
        if !kind.eq_ignore_ascii_case("button") {
            continue;
        }
        let Some(id) = attribute(tag, "id")
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .filter(|id| (1..10000).contains(id))
        else {
            continue;
        };
        if seen.insert(id) {
            ids.push(id);
        }
    }
    ids
}

fn named_ok(lower: &str) -> bool {
    lower == "ok.xml" || lower.ends_with("_ok.xml")
}

/// An add-on's busy overlay, named for progress. These windows open before
/// their labels do, and Back cancels the add-on's work. A results list is not
/// one of these.
pub(crate) fn busy_overlay_file(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    if lower.contains("notification") || lower.contains("result") {
        return false;
    }
    let plain: String = lower
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    plain.contains("progress")
        || plain.contains("resolve")
}

fn classify_skin(file: &str, text: &str, properties: &[String]) -> PromptKind {
    let lower = file.to_ascii_lowercase();
    if lower.contains("confirm") || lower.contains("yesno") {
        return PromptKind::Yesno;
    }
    if named_ok(&lower) {
        return PromptKind::Ok;
    }
    if busy_overlay_file(file) {
        return PromptKind::Progress;
    }
    let statusish = properties.iter().any(|name| {
        let n = name.to_ascii_lowercase();
        n.contains("status") || n == "percent"
    });
    if lower.contains("progress")
        || lower.contains("status")
        || lower.contains("resolve")
        || text.contains("type=\"progress\"")
        || statusish
    {
        return PromptKind::Progress;
    }
    // A window with no list that tells the viewer how to cancel ("Press BACK
    // to cancel") is the add-on at work, whatever the file is called.
    let lower_text = text.to_ascii_lowercase();
    if lower_text.contains("to cancel")
        && !lower_text.contains("type=\"list\"")
        && !lower_text.contains("type=\"panel\"")
    {
        return PromptKind::Progress;
    }
    if lower.contains("keyboard") || lower.contains("input") {
        return PromptKind::Text;
    }
    if lower.contains("select") || lower.contains("list") {
        return PromptKind::Select;
    }
    PromptKind::Ok
}

fn property_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let bytes = text.as_bytes();
    let needle = b"Property(";
    let mut i = 0;
    while i + needle.len() < bytes.len() {
        if !bytes[i..].starts_with(needle) {
            i += 1;
            continue;
        }
        let start = i + needle.len();
        let Some(end) = bytes[start..].iter().position(|&b| b == b')') else {
            break;
        };
        let raw = &text[start..start + end];
        let name = raw.trim().trim_matches(|c| c == '"' || c == '\'');
        if !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            && seen.insert(name.to_ascii_lowercase())
        {
            names.push(name.to_string());
        }
        i = start + end + 1;
    }
    names
}

/// Ids of label, textbox, and fadelabel controls outside list item layouts.
fn text_control_ids(text: &str) -> Vec<i64> {
    const TYPES: &[&str] = &["label", "textbox", "fadelabel"];
    let text = strip_comments(text);
    let text = cut_blocks(&text, "itemlayout");
    let text = cut_blocks(&text, "focusedlayout");
    let mut ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("<control") {
        let start = from + at;
        let Some(close) = text[start..].find('>') else {
            break;
        };
        let tag = &text[start..=start + close];
        from = start + close + 1;
        let Some(kind) = attribute(tag, "type") else {
            continue;
        };
        if !TYPES.iter().any(|name| kind.eq_ignore_ascii_case(name)) {
            continue;
        }
        let id = attribute(tag, "id")
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .filter(|id| (1..10000).contains(id))
            .or_else(|| {
                if tag.ends_with("/>") {
                    None
                } else {
                    child_control_id(control_body(&text, from))
                }
            });
        if let Some(id) = id {
            if seen.insert(id) {
                ids.push(id);
            }
        }
    }
    ids
}

fn cut_blocks(text: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&open) {
        out.push_str(&rest[..at]);
        match rest[at..].find(&close) {
            Some(end) => rest = &rest[at + end + close.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn child_control_id(body: &str) -> Option<i64> {
    let head = match body.find("<control") {
        Some(at) => &body[..at],
        None => body,
    };
    let start = head.find("<id>")?;
    let rest = head.get(start + 4..)?;
    let end = rest.find("</id>")?;
    let id = rest[..end].trim().parse::<i64>().ok()?;
    (1..10000).contains(&id).then_some(id)
}

fn control_ids(text: &str) -> Vec<i64> {
    let mut ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 < bytes.len() {
        if !(bytes[i] == b'i'
            && bytes.get(i + 1) == Some(&b'd')
            && (bytes.get(i + 2) == Some(&b'=')
                || (bytes.get(i + 2) == Some(&b' ') && bytes.get(i + 3) == Some(&b'='))))
        {
            i += 1;
            continue;
        }
        let eq = text[i..].find('=').map(|at| i + at).unwrap_or(i);
        let rest = text.get(eq + 1..).unwrap_or("");
        let rest = rest.trim_start();
        let rest = rest.trim_start_matches(['"', '\'']);
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(id) = digits.parse::<i64>() {
            if (1..10000).contains(&id) && seen.insert(id) {
                ids.push(id);
            }
        }
        i = eq + 1;
    }
    ids
}

/// The first list, panel, fixed list or wrap list outside XML comments.
fn list_control(text: &str) -> Option<SkinList> {
    const TYPES: &[&str] = &["list", "panel", "fixedlist", "wraplist"];
    let text = strip_comments(text);
    let mut from = 0;
    while let Some(at) = text[from..].find("<control") {
        let start = from + at;
        let close = text[start..].find('>')?;
        let tag = &text[start..=start + close];
        from = start + close + 1;
        let Some(kind) = attribute(tag, "type") else {
            continue;
        };
        if !TYPES.iter().any(|name| kind.eq_ignore_ascii_case(name)) || tag.ends_with("/>") {
            continue;
        }
        let Some(id) = attribute(tag, "id").and_then(|id| id.trim().parse::<i64>().ok()) else {
            continue;
        };
        let body = control_body(&text, from);
        let head = body
            .find("<itemlayout")
            .map_or(body, |layout| &body[..layout])
            .to_ascii_lowercase();
        let mut fields = item_fields(body);
        if fields.is_empty() {
            fields = vec!["Label".to_string(), "Label2".to_string()];
        }
        return Some(SkinList {
            id,
            horizontal: head.contains("<orientation>horizontal"),
            fields,
        });
    }
    None
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("<!--") {
        out.push_str(&rest[..open]);
        match rest[open..].find("-->") {
            Some(end) => rest = &rest[open + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(at) = tag[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        if !tag[..start].ends_with(char::is_whitespace) {
            continue;
        }
        let Some(value) = tag[from..].trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let quote = value
            .chars()
            .next()
            .filter(|ch| *ch == '"' || *ch == '\'')?;
        let inner = &value[1..];
        return inner.find(quote).map(|end| &inner[..end]);
    }
    None
}

/// Text from just after a control's opening tag to its closing tag.
fn control_body(text: &str, from: usize) -> &str {
    let mut depth = 1usize;
    let mut at = from;
    while depth > 0 {
        let open = text[at..].find("<control").map(|found| at + found);
        let Some(close) = text[at..].find("</control>").map(|found| at + found) else {
            return &text[from..];
        };
        match open {
            Some(open) if open < close => {
                let end = text[open..]
                    .find('>')
                    .map_or(text.len(), |found| open + found);
                if !text[open..end].ends_with('/') {
                    depth += 1;
                }
                at = (end + 1).min(text.len());
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    return &text[from..close];
                }
                at = close + "</control>".len();
            }
        }
    }
    &text[from..]
}

/// ListItem labels and properties a list's layout shows, in first-seen order
/// with the item's name first. Colors, art and raw links are left out.
fn item_fields(body: &str) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    let needle = "ListItem.";
    let mut from = 0;
    while let Some(at) = body[from..].find(needle) {
        let start = from + at + needle.len();
        from = start;
        let name: String = body[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        let field = match name.as_str() {
            "Label" | "Label2" | "Title" => name,
            "Property" => {
                let rest = &body[start + name.len()..];
                let Some(inner) = rest
                    .strip_prefix('(')
                    .and_then(|rest| rest.split(')').next())
                else {
                    continue;
                };
                let property = inner.trim();
                if property.is_empty() || !useful_item_property(property) {
                    continue;
                }
                format!("Property({property})")
            }
            _ => continue,
        };
        if !fields.contains(&field) {
            fields.push(field);
        }
    }
    let named = fields.iter().position(|field| {
        let lower = field.to_ascii_lowercase();
        lower == "label"
            || lower == "title"
            || ["name)", "title)", "label)"]
                .iter()
                .any(|end| lower.starts_with("property(") && lower.ends_with(end))
    });
    if let Some(named) = named {
        let first = fields.remove(named);
        fields.insert(0, first);
    }
    fields
}

fn useful_item_property(name: &str) -> bool {
    const SKIP: &[&str] = &[
        "color",
        "colour",
        "icon",
        "highlight",
        "thumb",
        "poster",
        "fanart",
        "logo",
        "image",
        "url",
        "path",
        "dict",
        "hash",
        "count",
    ];
    let lower = name.to_ascii_lowercase();
    !SKIP.iter().any(|part| lower.contains(part))
}

fn is_button_caption_property(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "ok_label"
        || lower == "yes_label"
        || lower == "no_label"
        || lower == "cancel_label"
        || lower.ends_with(".ok_label")
        || lower.ends_with(".yes_label")
        || lower.ends_with(".no_label")
        || lower.ends_with(".cancel_label")
        || lower.ends_with("_ok_label")
        || lower.ends_with("_yes_label")
        || lower.ends_with("_no_label")
        || lower.ends_with("_cancel_label")
}

fn is_button_caption_value(value: &str) -> bool {
    matches!(
        plain_markup(value).as_str(),
        "ok" | "okay" | "yes" | "no" | "cancel" | "close"
    )
}

fn plain_markup(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            for next in chars.by_ref() {
                if next == ']' {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out.trim().to_ascii_lowercase()
}

/// Prefer human title/message properties over color tokens and button captions.
/// A key that starts with a dot matches an add-on's namespaced property, such
/// as `<addon>.heading`.
pub fn pick_title(values: &[(String, String)]) -> String {
    const PREFERRED: &[&str] = &["heading", "title", "name", ".heading", ".label", "label"];
    for key in PREFERRED {
        if let Some((_, value)) = values.iter().find(|(name, _)| property_is(name, key)) {
            let trimmed = value.trim();
            if trimmed.is_empty() || is_button_caption_value(trimmed) {
                continue;
            }
            return trimmed.to_string();
        }
    }
    for (name, value) in values {
        let lower = name.to_ascii_lowercase();
        if lower.contains("color")
            || lower.contains("icon")
            || lower.contains("qr")
            || is_button_caption_property(name)
        {
            continue;
        }
        if lower.ends_with("heading") || lower.ends_with("title") {
            let trimmed = value.trim();
            if !trimmed.is_empty() && !is_button_caption_value(trimmed) {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

fn property_is(name: &str, key: &str) -> bool {
    if key.starts_with('.') {
        name.len() > key.len()
            && name.is_char_boundary(name.len() - key.len())
            && name[name.len() - key.len()..].eq_ignore_ascii_case(key)
    } else {
        name.eq_ignore_ascii_case(key)
    }
}

pub fn pick_message(values: &[(String, String)], title: &str) -> String {
    const PREFERRED: &[&str] = &[
        "text",
        "message",
        "content",
        "body",
        "line1",
        "line2",
        "status.main",
        "status.sub",
        "subtitle",
    ];
    let mut parts = Vec::new();
    for key in PREFERRED {
        if let Some((_, value)) = values
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
        {
            let trimmed = value.trim();
            if !trimmed.is_empty() && trimmed != title {
                parts.push(trimmed.to_string());
            }
        }
    }
    for (name, value) in values {
        let lower = name.to_ascii_lowercase();
        if lower.contains("color")
            || lower.contains("icon")
            || lower.contains("qr")
            || lower.contains("percent")
            || lower.ends_with("heading")
            || lower.ends_with("title")
            || is_button_caption_property(name)
        {
            continue;
        }
        if lower.ends_with("text")
            || lower.ends_with("message")
            || lower.ends_with("content")
            || lower.contains("status")
        {
            let trimmed = value.trim();
            if !trimmed.is_empty()
                && trimmed != title
                && !is_button_caption_value(trimmed)
                && !parts.iter().any(|part| part == trimmed)
            {
                parts.push(trimmed.to_string());
            }
        }
    }
    for (name, value) in values {
        if !is_control_label(name) {
            continue;
        }
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed == title
            || is_button_caption_value(trimmed)
            || is_control_chrome(trimmed)
            || parts.iter().any(|part| part == trimmed)
        {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    parts.join("\n")
}

fn is_control_label(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("control.getlabel(") && lower.ends_with(')')
}

fn is_control_chrome(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    if matches!(lower.as_str(), "progressbar" | "progress bar") {
        return true;
    }
    // An image control's description is a texture path, not dialog copy.
    if lower.contains('\n') || lower.contains(' ') {
        return false;
    }
    [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

pub fn pick_percent(values: &[(String, String)]) -> Option<f64> {
    values
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("percent"))
        .and_then(|(_, value)| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| {
            if (0.0..=1.0).contains(&value) {
                value
            } else {
                (value / 100.0).clamp(0.0, 1.0)
            }
        })
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/dialog_catalog.rs"]
mod tests;
