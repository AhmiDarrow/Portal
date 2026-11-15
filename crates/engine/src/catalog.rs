use crate::error::EngineError;
use crate::rpc::{KodiClient, RpcFail};
use portal_api::{AddonCard, AddonList, BrowseItem, BrowsePage, ItemKind, TitleKind};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::time::Instant;

const PLUGIN_TIMEOUT: Duration = Duration::from_secs(120);
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const FOLDER_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_PAUSE: Duration = Duration::from_millis(200);
const SEARCH_BUDGET: Duration = Duration::from_secs(25);
const POLL_PAUSE: Duration = Duration::from_millis(200);
const BROWSE_LIMIT: usize = 300;

// mpeg is listed before mpg so ".mpeg" is not consumed as ".mpg".
const MEDIA_EXTS: &[&[u8]] = &[
    b"mkv", b"mp4", b"avi", b"m4v", b"mov", b"flv", b"wmv", b"mpeg", b"mpg", b"mp3", b"flac",
    b"m4a", b"aac", b"ogg", b"wav",
];

const STYLE_TAGS: &[&str] = &[
    "COLOR",
    "UPPERCASE",
    "LOWERCASE",
    "CAPITALIZE",
    "LIGHT",
    "TABS",
];

pub fn plain_label(value: &str) -> String {
    plain_lines(value).join(" ")
}

/// Dialog copy keeps one line per row. Menu labels use `plain_label`.
pub fn plain_block(value: &str) -> String {
    plain_lines(value).join("\n")
}

fn plain_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let text = replace_ascii_token(value, "[CR]", "\n");
    let text = replace_ascii_token(&text, "[TABS]", " ");
    let text = strip_style_tags(&text);
    let text = strip_upper_tags(&text);
    let text = text.replace('\u{00A4}', " ");
    text.split(['\n', '\r'])
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect()
}

/// `item` is one Files.GetDirectory file object, or any JSON value.
/// Non-object values classify as ItemKind::Run.
pub fn classify(item: &serde_json::Value) -> portal_api::ItemKind {
    if !item.is_object() {
        return ItemKind::Run;
    }
    if item.get("filetype").and_then(Value::as_str) == Some("directory") {
        return ItemKind::Open;
    }
    let file = str_field(item, "file");
    if !is_plugin(file.trim()) {
        return ItemKind::Play;
    }
    if let Some(playable) = item.get("isplayable") {
        if is_json_true(playable) {
            return ItemKind::Play;
        }
        // isplayable false is an action, even when the label looks like a video.
        if is_json_false(playable) {
            return ItemKind::Run;
        }
    }
    let kind = str_field(item, "type");
    if matches!(
        kind.to_ascii_lowercase().as_str(),
        "movie" | "episode" | "musicvideo" | "song" | "video"
    ) {
        return ItemKind::Play;
    }
    if has_media_extension(str_field(item, "label"))
        || has_media_extension(str_field(item, "title"))
    {
        return ItemKind::Play;
    }
    if has_media_extension(&once_decoded(file)) {
        return ItemKind::Play;
    }
    ItemKind::Run
}

/// Add-on settings form to open instead of Player.Open, if this row is one.
/// `file` may be a bare plugin URL or a RunPlugin / ActivateWindow builtin.
pub fn settings_addon(file: &str, _label: &str) -> Option<String> {
    let action = plugin_query_action(file)?;
    if action.eq_ignore_ascii_case("openSettings") {
        return plugin_host(file).filter(|id| valid_addon_id(id));
    }
    None
}

/// Name of another add-on whose settings this row opens, read from an
/// action like `SomeAddonSettings` or a label like "Some Add-on : Settings".
/// The caller looks the name up among the installed add-ons.
pub fn settings_name_hint(file: &str, label: &str) -> Option<String> {
    let label = plain_label(label);
    let lower = label.to_ascii_lowercase();
    if let Some(head) = lower.strip_suffix("settings") {
        let name = label[..head.len()]
            .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '-' | '|'))
            .trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    let action = plugin_query_action(file)?;
    let head = action.len().checked_sub("settings".len())?;
    if !action.is_char_boundary(head) || !action[head..].eq_ignore_ascii_case("settings") {
        return None;
    }
    let name = action[..head].trim_end_matches(['_', '-', '.']);
    (!name.is_empty()).then(|| name.to_string())
}

/// Why `/api/play` must refuse this row. Settings open a form. A plugin tools
/// row must never call Player.Open; the page runs that row instead. A video
/// file is never refused because of its title.
pub fn play_refusal(file: &str, title: &str) -> Option<&'static str> {
    if settings_addon(file, title).is_some() {
        return Some("That item opens add-on settings.");
    }
    if !plugin_link(file) {
        return None;
    }
    let label = plain_label(title);
    if search_utility(&label) || people_action_label(&label) || utility_action_label(&label) {
        return Some("That item is an add-on action, not a video.");
    }
    None
}

/// A link Kodi runs as an add-on (`plugin://`, bare or inside a builtin).
pub fn plugin_link(file: &str) -> bool {
    plugin_span(file).is_some()
}

fn people_action_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower == "people" || lower.starts_with("people -") || lower.starts_with("people:")
}

/// Menu labels for tools/auth/cache/log rows when the query verb is missing.
fn utility_action_label(label: &str) -> bool {
    let lower = label
        .trim()
        .trim_end_matches('.')
        .trim()
        .to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "tools"
            | "help"
            | "accounts"
            | "account"
            | "clear cache"
            | "clean settings"
            | "changelog"
            | "log viewer"
            | "logging"
            | "authorize"
            | "authorise"
    ) || lower.starts_with("authorize ")
        || lower.starts_with("authorise ")
        || lower.starts_with("clear cache")
        || lower.ends_with(" manager")
        || lower.contains("upload log")
        || lower.contains("view log")
        || lower.contains("account info")
        || lower.contains("account manager")
        || lower.ends_with(" tools")
        || lower == "tools menu"
}

fn plugin_at(file: &str) -> Option<usize> {
    let needle = b"plugin://";
    let bytes = file.as_bytes();
    let mut index = 0;
    while index + needle.len() <= bytes.len() {
        if bytes[index..index + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(index);
        }
        let ch = file[index..].chars().next()?;
        index += ch.len_utf8();
    }
    None
}

fn plugin_span(file: &str) -> Option<&str> {
    plugin_at(file).map(|index| &file[index..])
}

fn stop_builtin(ch: char) -> bool {
    matches!(ch, ')' | '\'' | '"')
}

pub(crate) fn plugin_host(file: &str) -> Option<String> {
    let rest = plugin_span(file)?.get("plugin://".len()..)?;
    let mut host = String::new();
    for ch in rest.chars() {
        if ch == '/' || ch == '?' || stop_builtin(ch) {
            break;
        }
        host.push(ch);
    }
    Some(host)
}

fn plugin_query_action(file: &str) -> Option<String> {
    plugin_query_named(file, "action")
}

fn plugin_query_named(file: &str, key: &str) -> Option<String> {
    let span = plugin_span(file)?;
    let mut query_at: Option<usize> = None;
    for (offset, ch) in span.char_indices() {
        if stop_builtin(ch) {
            break;
        }
        if ch == '?' {
            query_at = Some(offset + 1);
            break;
        }
    }
    let start = query_at?;
    let mut end = span.len();
    for (offset, ch) in span[start..].char_indices() {
        if stop_builtin(ch) {
            end = start + offset;
            break;
        }
    }
    for pair in span[start..end].split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name.eq_ignore_ascii_case(key) {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn valid_addon_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 80 || id.contains("..") {
        return false;
    }
    id.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Path/query passed to `Addons.ExecuteAddon` for a `plugin://` URL.
fn plugin_execute_params(url: &str) -> String {
    let Some(span) = plugin_span(url) else {
        return String::new();
    };
    let Some(rest) = span.get("plugin://".len()..) else {
        return String::new();
    };
    let Some(index) = rest.find(['/', '?']) else {
        return String::new();
    };
    let mut out = String::new();
    for ch in rest[index..].chars() {
        if stop_builtin(ch) {
            break;
        }
        out.push(ch);
    }
    if out == "/" {
        String::new()
    } else {
        out
    }
}

/// Run a plugin URL through JSON-RPC `Addons.ExecuteAddon`.
/// A video plugin does not run `RunPlugin` on this path: Kodi opens it as a
/// directory and waits. Settings buttons go through the event server instead.
pub async fn execute_plugin(client: &KodiClient, url: &str) -> Result<(), EngineError> {
    let host = plugin_host(url)
        .filter(|id| valid_addon_id(id))
        .ok_or_else(|| EngineError::BadRequest("Unknown add-on.".into()))?;
    let params = plugin_execute_params(url);
    let mut body = json!({
        "addonid": host,
        "wait": false,
    });
    if !params.is_empty() {
        body["params"] = json!(params);
    }
    client
        .call("Addons.ExecuteAddon", body, Duration::from_secs(15))
        .await
        .map_err(EngineError::from)?;
    Ok(())
}

/// JSON object passed as the params of Files.GetDirectory.
/// Plugin paths use media "files". Other paths use media "video".
/// properties are "title", "thumbnail", "art", then the video info labels in
/// `kodi_info::INFO_PROPERTIES`. Never include isplayable.
pub fn directory_params(path: &str) -> serde_json::Value {
    let media = if is_plugin(path) { "files" } else { "video" };
    json!({
        "directory": path,
        "media": media,
        "properties": directory_properties(),
    })
}

fn directory_properties() -> Vec<&'static str> {
    let mut properties = vec!["title", "thumbnail", "art"];
    properties.extend_from_slice(crate::kodi_info::INFO_PROPERTIES);
    properties
}

pub async fn browse(
    client: &crate::rpc::KodiClient,
    path: &str,
) -> Result<portal_api::BrowsePage, crate::error::EngineError> {
    let plugin = is_plugin(path);
    match list_directory(client, path).await {
        Ok(result) if !plugin || directory_has_items(&result) => {
            return Ok(page_from(path, &result));
        }
        // A plugin error must not start a second GetDirectory. Empty Ok still follows.
        Err(err) => return Err(err.into()),
        _ => {}
    }

    // An empty plugin list may mean Kodi already opened a different folder.
    if let Some(follow) = follow_target(path, &container_folder(client).await) {
        return match list_directory(client, &follow).await {
            Ok(result) => Ok(page_from(&follow, &result)),
            Err(err) => Err(err.into()),
        };
    }

    tokio::time::sleep(RETRY_PAUSE).await;
    match list_directory(client, path).await {
        Ok(result) => Ok(page_from(path, &result)),
        Err(err) => Err(err.into()),
    }
}

pub async fn video_addons(
    client: &crate::rpc::KodiClient,
) -> Result<portal_api::AddonList, crate::error::EngineError> {
    let result = client
        .call(
            "Addons.GetAddons",
            json!({
                "type": "xbmc.addon.video",
                "enabled": true,
                "properties": ["name", "thumbnail"],
            }),
            LIST_TIMEOUT,
        )
        .await?;
    let mut items = Vec::new();
    if let Some(rows) = result.get("addons").and_then(Value::as_array) {
        for row in rows {
            if items.len() == BROWSE_LIMIT {
                break;
            }
            let Some(id) = row
                .get("addonid")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let name = row
                .get("name")
                .and_then(Value::as_str)
                .map(plain_label)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| id.to_string());
            let art = row
                .get("thumbnail")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string);
            items.push(AddonCard {
                id: id.to_string(),
                name,
                path: format!("plugin://{id}/"),
                art,
                removable: false,
            });
        }
    }
    Ok(AddonList { items })
}

pub async fn search(
    browse_client: &crate::rpc::KodiClient,
    dialog_client: &crate::rpc::KodiClient,
    settings: &Path,
    root: &str,
    query: &str,
) -> Result<portal_api::BrowsePage, crate::error::EngineError> {
    if query.trim().is_empty() {
        return Err(EngineError::BadRequest("Enter a search.".into()));
    }
    let root_page = browse(browse_client, root).await?;
    let Some(path) = search_root_path(&root_page) else {
        return search_unmarked_root(browse_client, dialog_client, settings, root_page, query)
            .await;
    };

    let (page, typed) = open_search(browse_client, dialog_client, &path, query).await?;
    if typed {
        return Ok(page);
    }
    let sections = search_section_paths(&page);
    if sections.is_empty() {
        return search_listing(browse_client, dialog_client, settings, page, query).await;
    }
    // A menu with a shows search and a movies search: a title can be either,
    // so both are searched and their results shown together.
    let mut found: Vec<BrowsePage> = Vec::new();
    let mut failed = None;
    for section in sections {
        let result = match open_search(browse_client, dialog_client, &section, query).await {
            Ok((nested, true)) => Ok(nested),
            Ok((nested, false)) => {
                search_listing(browse_client, dialog_client, settings, nested, query).await
            }
            Err(err) => Err(err),
        };
        match result {
            Ok(page) => found.push(page),
            Err(err) => failed = Some(err),
        }
    }
    match merge_results(found, query) {
        Some(page) => Ok(page),
        None => {
            Err(failed
                .unwrap_or_else(|| EngineError::BadRequest("This add-on has no search.".into())))
        }
    }
}

/// How one search listing should be asked. A folder route is browsed.
/// A Run row is started with RunPlugin; opening it as a directory is what
/// Kodi reports as invalid params.
#[derive(Debug)]
enum ListingSearch {
    Browse(String),
    Run(String),
    Open(String),
    Keep,
}

fn path_kind(page: &BrowsePage, path: &str) -> Option<ItemKind> {
    page.items
        .iter()
        .find(|item| item.path == path)
        .map(|item| item.kind)
}

fn listing_search_plan(page: &BrowsePage, query: &str) -> ListingSearch {
    if let Some((template, key)) = search_template(page) {
        if let Some(url) = with_query_value(&template, &key, query) {
            return if path_kind(page, &template) == Some(ItemKind::Run) {
                ListingSearch::Run(url)
            } else {
                ListingSearch::Browse(url)
            };
        }
    }
    if let Some(new_path) = new_search_path(page) {
        // A Run row opens a keyboard instead of a directory.
        return if path_kind(page, &new_path) == Some(ItemKind::Run) {
            ListingSearch::Run(new_path)
        } else {
            ListingSearch::Open(new_path)
        };
    }
    ListingSearch::Keep
}

/// Search when the root has no row named Search. Rows that are themselves a
/// search ("Search Movies...") are asked, shows first. A New search row is
/// started. A library folder named Movies is not a search.
async fn search_unmarked_root(
    browse_client: &KodiClient,
    dialog_client: &KodiClient,
    settings: &Path,
    page: BrowsePage,
    query: &str,
) -> Result<BrowsePage, EngineError> {
    let entries = root_search_entries(&page);
    if entries.is_empty() {
        if matches!(listing_search_plan(&page, query), ListingSearch::Keep) {
            return Err(EngineError::BadRequest("This add-on has no search.".into()));
        }
        return search_listing(browse_client, dialog_client, settings, page, query).await;
    }
    let mut found: Vec<BrowsePage> = Vec::new();
    let mut failed = None;
    for path in entries {
        let result = if path_kind(&page, &path) == Some(ItemKind::Run) {
            run_search_action(browse_client, dialog_client, settings, &path, query).await
        } else {
            match open_search(browse_client, dialog_client, &path, query).await {
                Ok((nested, true)) => Ok(nested),
                Ok((nested, false)) => {
                    search_listing(browse_client, dialog_client, settings, nested, query).await
                }
                Err(err) => Err(err),
            }
        };
        match result {
            Ok(page) => found.push(page),
            Err(err) => failed = Some(err),
        }
    }
    match merge_results(found, query) {
        Some(page) => Ok(page),
        None => Err(failed
            .unwrap_or_else(|| EngineError::BadRequest("This add-on has no search.".into()))),
    }
}

/// Search rows on a menu that has no row named Search. "Search history" is
/// not one, and neither is a library folder named Movies.
fn root_search_entries(page: &BrowsePage) -> Vec<String> {
    let mut rows: Vec<&BrowseItem> = page
        .items
        .iter()
        .filter(|item| is_root_search_entry(&item.label) && !item.path.trim().is_empty())
        .collect();
    rows.sort_by_key(|item| search_entry_rank(&item.label));
    let mut paths = Vec::new();
    for item in rows {
        if !paths.contains(&item.path) {
            paths.push(item.path.clone());
        }
    }
    paths
}

fn is_root_search_entry(label: &str) -> bool {
    let label = plain_label(label);
    let lower = label.trim().trim_end_matches('.').trim().to_ascii_lowercase();
    lower.starts_with("search")
        && lower != "search"
        && !lower.starts_with("search history")
        && !search_utility(&label)
}

/// Shows before movies, so a title is not buried under the other list.
fn search_entry_rank(label: &str) -> u8 {
    let lower = plain_label(label).to_ascii_lowercase();
    u8::from(!(lower.contains("show") || lower.contains("tv")))
}

/// Runs a search from one search listing: a saved search's own URL, or the
/// add-on's "New search" row.
async fn search_listing(
    browse_client: &KodiClient,
    dialog_client: &KodiClient,
    settings: &Path,
    page: BrowsePage,
    query: &str,
) -> Result<BrowsePage, EngineError> {
    match listing_search_plan(&page, query) {
        ListingSearch::Browse(url) => browse(browse_client, &url).await,
        ListingSearch::Run(url) => {
            run_search_action(browse_client, dialog_client, settings, &url, query).await
        }
        ListingSearch::Open(url) => {
            let (typed_page, _) = open_search(browse_client, dialog_client, &url, query).await?;
            Ok(typed_page)
        }
        ListingSearch::Keep => Ok(page),
    }
}

/// Start a search Run row, type the keyboard if one opens, then list the
/// folder the add-on switched to. The keyboard and the folder are polled
/// together so a direct results window is not held for the keyboard budget.
async fn run_search_action(
    browse_client: &KodiClient,
    dialog_client: &KodiClient,
    settings: &Path,
    path: &str,
    query: &str,
) -> Result<BrowsePage, EngineError> {
    // A hidden player never shows the search keyboard, and a busy dialog
    // often covers it. Keep the player on the work area until the add-on
    // switches to the results folder.
    let shown = crate::process::reveal_player_window();
    if let Some(window) = &shown {
        window.show_for_answer();
    }
    let before = container_folder(dialog_client).await;
    let builtin = run_plugin_builtin(path);
    if let Err(err) = crate::event::run_settings_builtin(settings, &builtin).await {
        drop(shown);
        return Err(err);
    }
    let started = Instant::now();
    let mut typed = false;
    let page = loop {
        if let Some(next) =
            changed_search_folder(&before, path, &container_folder(dialog_client).await)
        {
            break browse(browse_client, &next).await;
        }
        if !typed && keyboard_open(dialog_client).await {
            if let Some(window) = &shown {
                window.show_for_answer();
            }
            uncover_keyboard(dialog_client).await;
            // One try: a keyboard that will not take the text still has to
            // end within the search budget.
            let _ = dialog_client
                .call(
                    "Input.SendText",
                    json!({ "text": query, "done": true }),
                    Duration::from_secs(5),
                )
                .await;
            typed = true;
            continue;
        }
        let elapsed = started.elapsed();
        if elapsed >= SEARCH_BUDGET {
            break Err(EngineError::BadRequest("This add-on has no search.".into()));
        }
        tokio::time::sleep(POLL_PAUSE.min(SEARCH_BUDGET - elapsed)).await;
    };
    drop(shown);
    page
}

/// True when the keyboard dialog is the current window or is open under a
/// busy dialog. The busy window id alone is not a keyboard.
async fn keyboard_open(client: &KodiClient) -> bool {
    if let Ok(props) = client
        .call(
            "GUI.GetProperties",
            json!({ "properties": ["currentwindow"] }),
            Duration::from_secs(5),
        )
        .await
    {
        if keyboard_window(&props) {
            return true;
        }
    }
    const KEYS: [&str; 2] = [
        "Window.IsVisible(virtualkeyboard)",
        "Window.IsVisible(numericinput)",
    ];
    let Ok(flags) = client
        .call(
            "XBMC.GetInfoBooleans",
            json!({ "booleans": KEYS }),
            Duration::from_secs(5),
        )
        .await
    else {
        return false;
    };
    KEYS.iter().any(|key| info_flag(&flags, key))
}

/// Close a blank busy dialog sitting on a keyboard. The keyboard is what
/// can take the search text.
async fn uncover_keyboard(client: &KodiClient) {
    let Ok(props) = client
        .call(
            "GUI.GetProperties",
            json!({ "properties": ["currentwindow"] }),
            Duration::from_secs(5),
        )
        .await
    else {
        return;
    };
    let id = props
        .get("currentwindow")
        .and_then(|window| window.get("id"))
        .and_then(json_i64);
    if !matches!(id, Some(10101) | Some(10138) | Some(10160)) {
        return;
    }
    let _ = client
        .call(
            "Input.ExecuteAction",
            json!({ "action": "close" }),
            Duration::from_secs(5),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
}

/// `RunPlugin("<link>")`. Kodi splits builtin parameters on commas outside
/// quotes, so the link is quoted with its own quotes and backslashes escaped.
pub fn run_plugin_builtin(link: &str) -> String {
    let escaped = link.replace('\\', "\\\\").replace('"', "\\\"");
    format!("RunPlugin(\"{escaped}\")")
}

fn info_flag(value: &Value, key: &str) -> bool {
    match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => {
            let text = text.trim();
            text.eq_ignore_ascii_case("true") || text == "1"
        }
        Some(Value::Number(number)) => number.as_i64() == Some(1),
        _ => false,
    }
}

/// The container folder after a search Run, when it is a different plugin
/// from both the folder we left and the action that was run.
fn changed_search_folder(before: &str, ran: &str, folder: &str) -> Option<String> {
    if !is_plugin(folder) {
        return None;
    }
    let next = canonical_plugin(folder);
    if next.is_empty() || next == canonical_plugin(before) || next == canonical_plugin(ran) {
        return None;
    }
    let trimmed = folder.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// One page from the results of each section. A title whose name is what was
/// searched comes first, so a film is not buried under shows that only
/// mention it. Otherwise each section keeps the add-on's own order.
fn merge_results(pages: Vec<BrowsePage>, query: &str) -> Option<BrowsePage> {
    let mut pages = pages.into_iter();
    let mut merged = pages.next()?;
    for page in pages {
        for item in page.items {
            if !merged.items.iter().any(|seen| seen.path == item.path) {
                merged.items.push(item);
            }
        }
    }
    let wanted = query.trim().to_lowercase();
    let named = |item: &portal_api::BrowseItem| {
        let label = plain_label(&item.label).to_lowercase();
        let label = label.trim();
        // "Title (2010)" names the title too.
        label == wanted
            || label
                .strip_prefix(wanted.as_str())
                .is_some_and(|rest| rest.trim_start().starts_with('('))
    };
    let (first, rest): (Vec<_>, Vec<_>) = merged.items.into_iter().partition(|item| named(item));
    merged.items = first.into_iter().chain(rest).collect();
    Some(merged)
}

async fn open_search(
    browse_client: &KodiClient,
    dialog_client: &KodiClient,
    path: &str,
    query: &str,
) -> Result<(BrowsePage, bool), EngineError> {
    let listing = browse(browse_client, path);
    tokio::pin!(listing);
    tokio::select! {
        page = &mut listing => Ok((page?, false)),
        typed = type_search(dialog_client, query) => {
            // No keyboard and no listing in the whole budget. Waiting on would
            // hand back the search menu as if it were the results.
            if !typed {
                return Err(EngineError::Message(
                    "The add-on took too long to search.".into(),
                ));
            }
            Ok((listing.await?, true))
        }
    }
}

/// The root menu's Search row. Trailing dots ("Search...") are ignored, as
/// they are for the rows inside it.
fn search_root_path(page: &BrowsePage) -> Option<String> {
    page.items
        .iter()
        .find(|item| {
            item.label
                .trim()
                .trim_end_matches('.')
                .trim()
                .eq_ignore_ascii_case("search")
        })
        .map(|item| item.path.clone())
}

/// A saved search on a search listing: a row whose link carries its own
/// label as a value, which a new term can replace. Menu rows ("Search
/// Movies...") are never saved searches, whatever their links carry.
fn search_template(page: &BrowsePage) -> Option<(String, String)> {
    page.items.iter().find_map(|item| {
        let menu = plain_label(&item.label)
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("search");
        if menu || search_utility(&item.label) || item.path.trim().is_empty() {
            return None;
        }
        let key = query_key_for_label(&item.path, &item.label)?;
        Some((item.path.clone(), key))
    })
}

fn search_utility(label: &str) -> bool {
    let label = label.trim().trim_end_matches('.').trim();
    label.eq_ignore_ascii_case("new search")
        || label.eq_ignore_ascii_case("blacklist")
        || label.to_ascii_lowercase().contains("clear search history")
}

fn query_key_for_label(path: &str, label: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        // A bare flag such as `&refresh&` is skipped, not taken as the end.
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        // The action names a route, never the search term.
        if name.eq_ignore_ascii_case("action") {
            continue;
        }
        if !name.is_empty() && decode_query(value).eq_ignore_ascii_case(label.trim()) {
            return Some(name.to_string());
        }
    }
    None
}

fn with_query_value(path: &str, key: &str, query: &str) -> Option<String> {
    let (base, query_string) = path.split_once('?')?;
    let mut parts = Vec::new();
    let mut replaced = false;
    for pair in query_string.split('&') {
        let Some((name, _)) = pair.split_once('=') else {
            parts.push(pair.to_string());
            continue;
        };
        if name == key {
            parts.push(format!("{name}={}", encode_query(query.trim())));
            replaced = true;
        } else {
            parts.push(pair.to_string());
        }
    }
    if !replaced {
        return None;
    }
    Some(format!("{base}?{}", parts.join("&")))
}

/// Escapes decode to bytes first, so a term such as `caf%C3%A9` or a raw
/// `café` comes back as UTF-8 text rather than one char per byte.
fn decode_query(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'+' {
            out.push(b' ');
            index += 1;
            continue;
        }
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_nibble(bytes[index + 1]), hex_nibble(bytes[index + 2]))
            {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn new_search_path(page: &BrowsePage) -> Option<String> {
    page.items.iter().find_map(|item| {
        let label = item.label.trim().trim_end_matches('.').trim();
        if label.eq_ignore_ascii_case("new search") && !item.path.trim().is_empty() {
            Some(item.path.clone())
        } else {
            None
        }
    })
}

/// A search menu's shows section, then its movies section, as far as it has
/// them. Trailing dots ("Search Movies...") are ignored.
fn search_section_paths(page: &BrowsePage) -> Vec<String> {
    const SHOWS: [&str; 3] = ["TV Shows", "Search Shows", "Search TV Shows"];
    const MOVIES: [&str; 2] = ["Movies", "Search Movies"];
    let mut paths = Vec::new();
    for names in [&SHOWS[..], &MOVIES[..]] {
        let found = names.iter().find_map(|name| {
            page.items.iter().find(|item| {
                item.kind == ItemKind::Open
                    && !item.path.trim().is_empty()
                    && item
                        .label
                        .trim()
                        .trim_end_matches('.')
                        .trim()
                        .eq_ignore_ascii_case(name)
            })
        });
        if let Some(item) = found {
            if !paths.contains(&item.path) {
                paths.push(item.path.clone());
            }
        }
    }
    paths
}

fn replace_ascii_token(input: &str, token: &str, replacement: &str) -> String {
    let token_bytes = token.as_bytes();
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + token_bytes.len() <= bytes.len()
            && bytes[index..index + token_bytes.len()].eq_ignore_ascii_case(token_bytes)
        {
            out.push_str(replacement);
            index += token_bytes.len();
            continue;
        }
        let ch = input[index..]
            .chars()
            .next()
            .expect("index is on a char boundary");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn strip_style_tags(input: &str) -> String {
    strip_tags(input, style_tag_end)
}

fn strip_upper_tags(input: &str) -> String {
    strip_tags(input, upper_tag_end)
}

fn strip_tags(input: &str, mut tag_end: impl FnMut(&[char], usize) -> Option<usize>) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '[' {
            if let Some(end) = tag_end(&chars, index) {
                index = end;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn style_tag_end(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if index < chars.len() && chars[index] == '/' {
        index += 1;
    }
    for name in STYLE_TAGS {
        if starts_with_ascii_ignore(chars, index, name) {
            let mut end = index + name.len();
            while end < chars.len() && chars[end] != ']' {
                end += 1;
            }
            if end < chars.len() && chars[end] == ']' {
                return Some(end + 1);
            }
            return None;
        }
    }
    if index < chars.len()
        && (chars[index].eq_ignore_ascii_case(&'B') || chars[index].eq_ignore_ascii_case(&'I'))
        && index + 1 < chars.len()
        && chars[index + 1] == ']'
    {
        return Some(index + 2);
    }
    None
}

fn upper_tag_end(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if index < chars.len() && chars[index] == '/' {
        index += 1;
    }
    if index >= chars.len() || !chars[index].is_ascii_uppercase() {
        return None;
    }
    index += 1;
    while index < chars.len()
        && (chars[index].is_ascii_uppercase() || chars[index].is_ascii_digit())
    {
        index += 1;
    }
    if index < chars.len() && chars[index] == ' ' {
        index += 1;
        while index < chars.len() && chars[index] != ']' {
            index += 1;
        }
    }
    if index < chars.len() && chars[index] == ']' {
        Some(index + 1)
    } else {
        None
    }
}

fn starts_with_ascii_ignore(chars: &[char], start: usize, name: &str) -> bool {
    for (index, want) in (start..).zip(name.chars()) {
        if index >= chars.len() || !chars[index].eq_ignore_ascii_case(&want) {
            return false;
        }
    }
    true
}

fn str_field<'a>(item: &'a Value, key: &str) -> &'a str {
    item.get(key).and_then(Value::as_str).unwrap_or("")
}

fn is_plugin(path: &str) -> bool {
    let trimmed = path.trim_start();
    let prefix = b"plugin://";
    trimmed.len() >= prefix.len() && trimmed.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn is_json_true(value: &Value) -> bool {
    value.as_bool() == Some(true) || value.as_str() == Some("true")
}

fn is_json_false(value: &Value) -> bool {
    value.as_bool() == Some(false) || value.as_str() == Some("false")
}

fn has_media_extension(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'.' {
            continue;
        }
        for ext in MEDIA_EXTS {
            let end = index + 1 + ext.len();
            if end <= bytes.len() && &bytes[index + 1..end] == *ext && media_delimiter(&lower, end)
            {
                return true;
            }
        }
    }
    false
}

fn media_delimiter(text: &str, index: usize) -> bool {
    if index >= text.len() {
        return true;
    }
    match text[index..].chars().next() {
        Some(ch) => matches!(ch, '?' | '#' | '&') || ch.is_whitespace(),
        None => true,
    }
}

// A bad % escape rejects the whole string, the same way decodeURIComponent does.
fn once_decoded(url: &str) -> String {
    if bad_percent(url) {
        return url.to_string();
    }
    let bytes = url.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(high) = bytes.get(index + 1).copied().and_then(hex_value) else {
                return url.to_string();
            };
            let Some(low) = bytes.get(index + 2).copied().and_then(hex_value) else {
                return url.to_string();
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| url.to_string())
}

fn bad_percent(url: &str) -> bool {
    let bytes = url.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || hex_value(bytes[index + 1]).is_none()
                || hex_value(bytes[index + 2]).is_none()
            {
                return true;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    false
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

async fn list_directory(client: &KodiClient, path: &str) -> Result<Value, RpcFail> {
    let limit = if is_plugin(path) {
        PLUGIN_TIMEOUT
    } else {
        LIST_TIMEOUT
    };
    client
        .call("Files.GetDirectory", directory_params(path), limit)
        .await
}

async fn container_folder(client: &KodiClient) -> String {
    let Ok(value) = client
        .call(
            "XBMC.GetInfoLabels",
            json!({ "labels": ["Container.FolderPath"] }),
            FOLDER_TIMEOUT,
        )
        .await
    else {
        return String::new();
    };
    value
        .get("Container.FolderPath")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn follow_target(requested: &str, folder: &str) -> Option<String> {
    if !is_plugin(requested) || !is_plugin(folder) {
        return None;
    }
    let next = canonical_plugin(folder);
    if next.is_empty() || next == canonical_plugin(requested) {
        return None;
    }
    let trimmed = folder.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn canonical_plugin(url: &str) -> String {
    once_decoded(url.trim()).trim_end_matches('/').to_string()
}

fn directory_has_items(result: &Value) -> bool {
    match result.get("files").and_then(Value::as_array) {
        Some(rows) => rows.iter().any(Value::is_object),
        None => false,
    }
}

fn page_from(path: &str, result: &Value) -> BrowsePage {
    let items = result
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.is_object())
        .take(BROWSE_LIMIT)
        .map(browse_item)
        .collect();
    BrowsePage {
        path: path.to_string(),
        items,
    }
}

fn browse_item(item: &Value) -> BrowseItem {
    let label = item_label(item);
    let file = item.get("file").and_then(Value::as_str);
    let (id, path) = match file {
        Some(file) => (file.to_string(), file.to_string()),
        None => (label.clone(), String::new()),
    };
    let (season, episode) = season_episode(item, &label);
    let title_kind = title_kind(item, &label);
    let settings_addon = settings_addon(&path, &label);
    BrowseItem {
        id,
        label,
        path,
        kind: classify(item),
        art: item_art(item),
        title_kind,
        season,
        episode,
        show_title: show_title(item),
        show_path: None,
        settings_addon,
        info: crate::kodi_info::media_info(item),
    }
}

fn item_label(item: &Value) -> String {
    for key in ["label", "title"] {
        if let Some(text) = item.get(key).and_then(Value::as_str) {
            let plain = plain_label(text);
            if !plain.is_empty() {
                return plain;
            }
        }
    }
    String::new()
}

fn item_art(item: &Value) -> Option<String> {
    if let Some(thumb) = nonempty_str(item.get("thumbnail")) {
        return Some(thumb);
    }
    let art = item.get("art")?;
    if let Some(poster) = nonempty_str(art.get("poster")) {
        return Some(poster);
    }
    nonempty_str(art.get("thumb"))
}

fn nonempty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn title_kind(item: &Value, label: &str) -> Option<TitleKind> {
    match str_field(item, "type").to_ascii_lowercase().as_str() {
        "movie" => Some(TitleKind::Movie),
        "episode" | "tvshow" => Some(TitleKind::Tv),
        _ if parse_season_episode(label).is_some() => Some(TitleKind::Tv),
        _ => None,
    }
}

fn show_title(item: &Value) -> Option<String> {
    item.get("showtitle")
        .and_then(Value::as_str)
        .map(plain_label)
        .filter(|text| !text.is_empty())
}

fn season_episode(item: &Value, label: &str) -> (Option<u32>, Option<u32>) {
    let mut season = item.get("season").and_then(season_number);
    let mut episode = item.get("episode").and_then(positive_u32);
    if season.is_none() || episode.is_none() {
        if let Some((parsed_season, parsed_episode)) = parse_season_episode(label) {
            if season.is_none() {
                season = Some(parsed_season);
            }
            if episode.is_none() && parsed_episode > 0 {
                episode = Some(parsed_episode);
            }
        }
    }
    // Season 0 is a special only when an episode is present. A bare 0 is a missing tag.
    if season == Some(0) && episode.is_none() {
        season = None;
    }
    (season, episode)
}

fn season_number(value: &Value) -> Option<u32> {
    let number = value.as_i64()?;
    if number >= 0 {
        u32::try_from(number).ok()
    } else {
        None
    }
}

fn positive_u32(value: &Value) -> Option<u32> {
    let number = value.as_i64()?;
    if number > 0 {
        u32::try_from(number).ok()
    } else {
        None
    }
}

fn parse_season_episode(label: &str) -> Option<(u32, u32)> {
    let chars: Vec<char> = label.chars().collect();
    find_coded(&chars).or_else(|| find_crossed(&chars))
}

fn find_coded(chars: &[char]) -> Option<(u32, u32)> {
    let mut index = 0;
    while index < chars.len() {
        if chars[index].eq_ignore_ascii_case(&'s') && left_boundary(chars, index) {
            let season_end = take_digits(chars, index + 1);
            if season_end > index + 1
                && season_end < chars.len()
                && chars[season_end].eq_ignore_ascii_case(&'e')
            {
                let episode_end = take_digits(chars, season_end + 1);
                if episode_end > season_end + 1 && right_boundary(chars, episode_end) {
                    if let (Some(season), Some(episode)) = (
                        parse_u32_digits(&chars[index + 1..season_end]),
                        parse_u32_digits(&chars[season_end + 1..episode_end]),
                    ) {
                        return Some((season, episode));
                    }
                }
            }
        }
        index += 1;
    }
    None
}

fn find_crossed(chars: &[char]) -> Option<(u32, u32)> {
    let mut index = 0;
    while index < chars.len() {
        if chars[index].is_ascii_digit() && left_boundary(chars, index) {
            let season_end = take_digits(chars, index);
            if season_end < chars.len() && chars[season_end].eq_ignore_ascii_case(&'x') {
                let episode_end = take_digits(chars, season_end + 1);
                if episode_end > season_end + 1 && right_boundary(chars, episode_end) {
                    if let (Some(season), Some(episode)) = (
                        parse_u32_digits(&chars[index..season_end]),
                        parse_u32_digits(&chars[season_end + 1..episode_end]),
                    ) {
                        return Some((season, episode));
                    }
                }
            }
        }
        index += 1;
    }
    None
}

fn take_digits(chars: &[char], start: usize) -> usize {
    let mut cursor = start;
    while cursor < chars.len() && chars[cursor].is_ascii_digit() {
        cursor += 1;
    }
    cursor
}

fn parse_u32_digits(chars: &[char]) -> Option<u32> {
    if chars.is_empty() {
        return None;
    }
    let text: String = chars.iter().collect();
    text.parse().ok()
}

fn left_boundary(chars: &[char], index: usize) -> bool {
    index == 0 || !chars[index - 1].is_alphanumeric()
}

fn right_boundary(chars: &[char], index: usize) -> bool {
    index >= chars.len() || !chars[index].is_alphanumeric()
}

// 10103 is the keyboard dialog and 10109 is the numeric dialog.
// Input.Back is backspace there, so this loop never sends it.
// Returns false when the budget ran out before a keyboard opened.
async fn type_search(dialog: &KodiClient, query: &str) -> bool {
    let started = Instant::now();
    loop {
        let elapsed = started.elapsed();
        if elapsed >= SEARCH_BUDGET {
            return false;
        }
        let limit = Duration::from_secs(5).min(SEARCH_BUDGET - elapsed);
        if let Ok(props) = dialog
            .call(
                "GUI.GetProperties",
                json!({ "properties": ["currentwindow"] }),
                limit,
            )
            .await
        {
            if keyboard_window(&props) {
                let shown = crate::process::reveal_player_window();
                if let Some(window) = &shown {
                    window.show_for_answer();
                }
                uncover_keyboard(dialog).await;
                let _ = dialog
                    .call(
                        "Input.SendText",
                        json!({ "text": query, "done": true }),
                        Duration::from_secs(5),
                    )
                    .await;
                tokio::time::sleep(Duration::from_millis(400)).await;
                drop(shown);
                return true;
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= SEARCH_BUDGET {
            return false;
        }
        tokio::time::sleep(POLL_PAUSE.min(SEARCH_BUDGET - elapsed)).await;
    }
}

fn keyboard_window(props: &Value) -> bool {
    let id = props
        .get("currentwindow")
        .and_then(|window| window.get("id"))
        .and_then(json_i64);
    id == Some(10103) || id == Some(10109)
}

fn json_i64(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    value.as_str().and_then(|text| text.parse().ok())
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/catalog.rs"]
mod tests;
