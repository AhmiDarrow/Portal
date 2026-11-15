//! Add-on settings forms. Values stay in Kodi's addon_data file. Secrets sent
//! to the browser are blank, and an empty secret write does not erase the saved one.

use crate::catalog::plain_label;
use crate::error::EngineError;
use portal_api::{FieldKind, SelectOption, SettingsCategory, SettingsDoc, SettingsField};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Settings {
    profile: PathBuf,
    programs: PathBuf,
}

impl Settings {
    pub fn new(profile: impl Into<PathBuf>, programs_addons: impl Into<PathBuf>) -> Self {
        Self {
            profile: profile.into(),
            programs: programs_addons.into(),
        }
    }

    pub fn form(&self, addon_id: &str) -> Result<SettingsDoc, EngineError> {
        let addon_id = check_id(addon_id)?;
        let dir = self.addon_dir(&addon_id)?;
        let definition = fs::read_to_string(dir.join("resources").join("settings.xml"))
            .map_err(|_| EngineError::NotFound)?;
        let strings = load_strings(&dir);
        let categories = parse_settings(&definition, &strings)?;
        let saved = read_saved(&self.saved_path(&addon_id));
        Ok(SettingsDoc {
            addon_id,
            categories: public_form(&categories, &saved),
        })
    }

    pub fn write_value(
        &self,
        addon_id: &str,
        setting_id: &str,
        value: &str,
    ) -> Result<(), EngineError> {
        let addon_id = check_id(addon_id)?;
        let setting_id = setting_id.trim();
        if setting_id.is_empty() || setting_id.contains(['/', '\\']) || setting_id.contains("..") {
            return Err(EngineError::BadRequest("Unknown setting.".into()));
        }
        let dir = self.addon_dir(&addon_id)?;
        let definition = fs::read_to_string(dir.join("resources").join("settings.xml"))
            .map_err(|_| EngineError::NotFound)?;
        let categories = parse_settings(&definition, &BTreeMap::new())?;
        let field = find_field(&categories, setting_id)
            .ok_or_else(|| EngineError::BadRequest("Unknown setting.".into()))?;
        if field.kind == RawKind::Header || field.id.is_none() {
            return Err(EngineError::BadRequest(
                "That row cannot be changed.".into(),
            ));
        }
        if field.kind == RawKind::Action {
            return Err(EngineError::BadRequest("That row is an action.".into()));
        }
        if field.secret && value.trim().is_empty() {
            return Ok(());
        }
        let stored = if field.kind == RawKind::Bool {
            as_bool(value)
        } else {
            value.to_string()
        };
        let path = self.saved_path(&addon_id);
        write_saved(&path, setting_id, &stored)
    }

    /// True when an add-on by this id is installed, in the profile or with Kodi.
    pub fn addon_installed(&self, addon_id: &str) -> bool {
        check_id(addon_id).is_ok_and(|id| {
            self.profile.join("addons").join(&id).join("addon.xml").is_file()
                || self.programs.join(&id).join("addon.xml").is_file()
        })
    }

    /// The one installed add-on with a settings form whose name matches
    /// `name`, ignoring case, spaces and punctuation.
    pub fn addon_named(&self, name: &str) -> Option<String> {
        let wanted = name_key(name);
        if wanted.is_empty() {
            return None;
        }
        let mut found: Vec<String> = Vec::new();
        for root in [self.profile.join("addons"), self.programs.clone()] {
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.join("resources").join("settings.xml").is_file() {
                    continue;
                }
                let Ok(xml) = fs::read_to_string(dir.join("addon.xml")) else {
                    continue;
                };
                let Some(addon_name) = crate::addons::addon_name(&xml) else {
                    continue;
                };
                if name_key(&plain_label(&addon_name)) != wanted {
                    continue;
                }
                if let Some(id) = entry.file_name().to_str() {
                    if check_id(id).is_ok() && !found.iter().any(|seen| seen == id) {
                        found.push(id.to_string());
                    }
                }
            }
        }
        // Two add-ons with the same name leave the row an ordinary action.
        if found.len() == 1 {
            found.pop()
        } else {
            None
        }
    }

    pub fn action_builtin(&self, addon_id: &str, setting_id: &str) -> Result<String, EngineError> {
        let addon_id = check_id(addon_id)?;
        let dir = self.addon_dir(&addon_id)?;
        let definition = fs::read_to_string(dir.join("resources").join("settings.xml"))
            .map_err(|_| EngineError::NotFound)?;
        let categories = parse_settings(&definition, &BTreeMap::new())?;
        let field = find_field(&categories, setting_id)
            .ok_or_else(|| EngineError::BadRequest("Unknown setting.".into()))?;
        if field.kind != RawKind::Action {
            return Err(EngineError::BadRequest("That row is not an action.".into()));
        }
        let action = field.action.trim();
        if !(action.starts_with("RunPlugin(") || action.starts_with("ActivateWindow(")) {
            return Err(EngineError::BadRequest(
                "That action is not available.".into(),
            ));
        }
        Ok(action.to_string())
    }

    fn addon_dir(&self, addon_id: &str) -> Result<PathBuf, EngineError> {
        let profile = self.profile.join("addons").join(addon_id);
        let programs = self.programs.join(addon_id);
        if profile.join("resources").join("settings.xml").is_file() {
            Ok(profile)
        } else if programs.join("resources").join("settings.xml").is_file() {
            Ok(programs)
        } else {
            Err(EngineError::NotFound)
        }
    }

    fn saved_path(&self, addon_id: &str) -> PathBuf {
        self.profile
            .join("userdata")
            .join("addon_data")
            .join(addon_id)
            .join("settings.xml")
    }


}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawKind {
    Bool,
    Text,
    Number,
    Select,
    Action,
    Header,
}

struct RawField {
    id: Option<String>,
    label: String,
    kind: RawKind,
    default_value: String,
    options: Vec<SelectOption>,
    secret: bool,
    visible_expr: String,
    enable_expr: String,
    action: String,
    /// A version-1 `<group>` title is not a setting. `eq(-1)` still means the
    /// previous setting, while an `lsep` row keeps its place.
    eq_slot: bool,
}

struct RawCategory {
    label: String,
    fields: Vec<RawField>,
}

/// Lowercase letters and digits only, so "Some Add-on" matches "someaddon".
fn name_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn check_id(addon_id: &str) -> Result<String, EngineError> {
    let id = addon_id.trim();
    let ok = (1..=80).contains(&id.len())
        && !id.contains("..")
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-');
    if !ok {
        return Err(EngineError::BadRequest("Unknown add-on.".into()));
    }
    Ok(id.to_string())
}

fn find_field<'a>(categories: &'a [RawCategory], id: &str) -> Option<&'a RawField> {
    categories
        .iter()
        .flat_map(|category| category.fields.iter())
        .find(|field| field.id.as_deref() == Some(id))
}

fn public_form(
    categories: &[RawCategory],
    saved: &BTreeMap<String, String>,
) -> Vec<SettingsCategory> {
    let mut by_id: BTreeMap<&str, &RawField> = BTreeMap::new();
    for category in categories {
        for field in &category.fields {
            if let Some(id) = field.id.as_deref() {
                by_id.entry(id).or_insert(field);
            }
        }
    }
    let mut shown = Vec::new();
    for category in categories {
        let mut fields = Vec::new();
        for (index, field) in category.fields.iter().enumerate() {
            if !condition(&field.visible_expr, &category.fields, index, saved, &by_id) {
                continue;
            }
            // A separator whose label is only [CR] or an empty color tag still
            // occupies an eq() slot above, but it is not a heading.
            if field.kind == RawKind::Header && field.label.is_empty() {
                continue;
            }
            let enabled = condition(&field.enable_expr, &category.fields, index, saved, &by_id);
            fields.push(SettingsField {
                id: field.id.clone(),
                kind: public_kind(field),
                label: field.label.clone(),
                value: public_value(field, saved),
                options: field.options.clone(),
                secret: field.secret,
                enabled,
                action: if field.kind == RawKind::Action {
                    field.action.clone()
                } else {
                    String::new()
                },
            });
        }
        if !fields.is_empty() {
            shown.push(SettingsCategory {
                label: category.label.clone(),
                fields,
            });
        }
    }
    shown
}

fn public_kind(field: &RawField) -> FieldKind {
    if field.secret && matches!(field.kind, RawKind::Text | RawKind::Number) {
        return FieldKind::Secret;
    }
    match field.kind {
        RawKind::Bool => FieldKind::Bool,
        RawKind::Text => FieldKind::Text,
        RawKind::Number => FieldKind::Number,
        RawKind::Select => FieldKind::Select,
        RawKind::Action => FieldKind::Action,
        RawKind::Header => FieldKind::Header,
    }
}

fn public_value(field: &RawField, saved: &BTreeMap<String, String>) -> String {
    if field.secret || field.kind == RawKind::Header {
        return String::new();
    }
    effective(field, saved)
}

fn effective(field: &RawField, saved: &BTreeMap<String, String>) -> String {
    let raw = field
        .id
        .as_deref()
        .and_then(|id| saved.get(id))
        .cloned()
        .unwrap_or_else(|| field.default_value.clone());
    if field.kind == RawKind::Bool {
        as_bool(&raw)
    } else {
        raw
    }
}

fn as_bool(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => "true".to_string(),
        _ => "false".to_string(),
    }
}

fn condition(
    expr: &str,
    fields: &[RawField],
    index: usize,
    saved: &BTreeMap<String, String>,
    by_id: &BTreeMap<&str, &RawField>,
) -> bool {
    eval_expr(expr.trim(), fields, index, saved, by_id).unwrap_or(true)
}

fn eval_expr(
    expr: &str,
    fields: &[RawField],
    index: usize,
    saved: &BTreeMap<String, String>,
    by_id: &BTreeMap<&str, &RawField>,
) -> Option<bool> {
    let text = strip_parens(expr.trim());
    if text.is_empty() || text.eq_ignore_ascii_case("true") {
        return Some(true);
    }
    if text.eq_ignore_ascii_case("false") {
        return Some(false);
    }
    if text.contains("System.") {
        return None;
    }
    let or_parts = split_top(text, '|');
    if or_parts.len() > 1 {
        let mut any = false;
        for part in or_parts {
            any |= eval_expr(part, fields, index, saved, by_id)?;
        }
        return Some(any);
    }
    let and_parts = split_top(text, '+');
    if and_parts.len() > 1 {
        let mut all = true;
        for part in and_parts {
            all &= eval_expr(part, fields, index, saved, by_id)?;
        }
        return Some(all);
    }
    eval_atom(text, fields, index, saved, by_id)
}

fn strip_parens(text: &str) -> &str {
    let text = text.trim();
    if text.len() >= 2 && text.starts_with('(') && text.ends_with(')') && wraps_fully(text) {
        return strip_parens(&text[1..text.len() - 1]);
    }
    text
}

fn wraps_fully(text: &str) -> bool {
    let mut depth = 0i32;
    // Byte offsets, to compare with `text.len()` when a value is not ASCII.
    for (index, ch) in text.char_indices() {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return index == text.len() - 1;
            }
            if depth < 0 {
                return false;
            }
        }
    }
    false
}

fn eval_atom(
    text: &str,
    fields: &[RawField],
    index: usize,
    saved: &BTreeMap<String, String>,
    by_id: &BTreeMap<&str, &RawField>,
) -> Option<bool> {
    if text.eq_ignore_ascii_case("true") {
        return Some(true);
    }
    if text.eq_ignore_ascii_case("false") {
        return Some(false);
    }
    if let Some((neg, offset, expected)) = parse_eq(text) {
        if offset >= 0 {
            return None;
        }
        let slot = eq_slot_index(fields, index)?;
        let target = slot as i32 + offset;
        if target < 0 {
            return Some(false);
        }
        let Some(field) = field_at_eq_slot(fields, target as usize) else {
            return Some(false);
        };
        let equals = values_equal(field, &effective(field, saved), expected);
        return Some(if neg { !equals } else { equals });
    }
    if let Some((neg, id, expected)) = parse_is(text) {
        let equals = if let Some(field) = by_id.get(id) {
            values_equal(field, &effective(field, saved), expected)
        } else {
            saved.get(id).map(String::as_str).unwrap_or("") == expected
        };
        return Some(if neg { !equals } else { equals });
    }
    None
}

fn values_equal(field: &RawField, actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }
    if field.kind == RawKind::Select
        && expected.chars().all(|ch| ch.is_ascii_digit())
        && !expected.is_empty()
    {
        let wanted: usize = match expected.parse() {
            Ok(value) => value,
            Err(_) => return false,
        };
        return field
            .options
            .iter()
            .position(|option| option.value == actual || option.label == actual)
            == Some(wanted);
    }
    false
}

fn eq_slot_index(fields: &[RawField], index: usize) -> Option<usize> {
    if !fields.get(index)?.eq_slot {
        return None;
    }
    Some(
        fields
            .iter()
            .take(index)
            .filter(|field| field.eq_slot)
            .count(),
    )
}

fn field_at_eq_slot(fields: &[RawField], slot: usize) -> Option<&RawField> {
    fields.iter().filter(|field| field.eq_slot).nth(slot)
}

fn parse_eq(text: &str) -> Option<(bool, i32, &str)> {
    let (neg, rest) = peel_bang(text);
    let rest = rest.trim();
    let rest = rest
        .strip_prefix("eq(")
        .or_else(|| rest.strip_prefix("EQ("))?;
    let rest = rest.strip_suffix(')')?;
    let (offset, value) = rest.split_once(',')?;
    let offset = offset.trim().parse::<i32>().ok()?;
    Some((neg, offset, value.trim()))
}

fn parse_is(text: &str) -> Option<(bool, &str, &str)> {
    let (neg, rest) = peel_bang(text);
    let rest = rest.trim();
    let rest = rest
        .strip_prefix("is(")
        .or_else(|| rest.strip_prefix("IS("))?;
    let rest = rest.strip_suffix(')')?;
    let (id, value) = rest.split_once(',')?;
    Some((neg, id.trim(), value.trim()))
}

fn peel_bang(text: &str) -> (bool, &str) {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix('!') {
        (true, rest.trim())
    } else {
        (false, trimmed)
    }
}

fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    for (index, ch) in text.char_indices() {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' && depth > 0 {
            depth -= 1;
        } else if ch == sep && depth == 0 {
            let part = text[start..index].trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = index + ch.len_utf8();
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

fn parse_settings(
    xml: &str,
    strings: &BTreeMap<i64, String>,
) -> Result<Vec<RawCategory>, EngineError> {
    let root = parse_xml(xml)
        .map_err(|_| EngineError::Message("Could not read these add-on settings.".into()))?;
    let mut categories: Vec<&XmlNode> = Vec::new();
    collect_named(&root, "category", &mut categories);
    if categories.is_empty() && root.name == "category" {
        categories.push(&root);
    }
    if categories.is_empty() {
        let fields = fields_under(&root, strings);
        if fields.is_empty() {
            return Ok(Vec::new());
        }
        return Ok(vec![RawCategory {
            label: String::new(),
            fields,
        }]);
    }
    Ok(categories
        .into_iter()
        .map(|category| RawCategory {
            label: localize(category.attr("label").unwrap_or(""), strings),
            fields: fields_under(category, strings),
        })
        .collect())
}

fn collect_named<'a>(node: &'a XmlNode, name: &str, out: &mut Vec<&'a XmlNode>) {
    if node.name == name && node.name != "settings" {
        out.push(node);
    }
    for child in &node.children {
        if child.name == name {
            out.push(child);
        } else if child.name != "category" {
            collect_named(child, name, out);
        }
    }
}

fn fields_under(parent: &XmlNode, strings: &BTreeMap<i64, String>) -> Vec<RawField> {
    let mut found = Vec::new();
    append_fields(parent, strings, &mut found);
    found
}

fn append_fields(node: &XmlNode, strings: &BTreeMap<i64, String>, found: &mut Vec<RawField>) {
    for child in &node.children {
        if child.name == "category" {
            continue;
        }
        if child.name == "setting" {
            found.push(parse_setting(child, strings));
        } else if child.name == "group" {
            let label = localize(child.attr("label").unwrap_or(""), strings);
            if !label.is_empty() {
                found.push(group_header(label));
            }
            append_fields(child, strings, found);
        } else {
            append_fields(child, strings, found);
        }
    }
}

fn group_header(label: String) -> RawField {
    RawField {
        id: None,
        label,
        kind: RawKind::Header,
        default_value: String::new(),
        options: Vec::new(),
        secret: false,
        visible_expr: String::new(),
        enable_expr: String::new(),
        action: String::new(),
        eq_slot: false,
    }
}

fn parse_setting(node: &XmlNode, strings: &BTreeMap<i64, String>) -> RawField {
    let setting_type = node.attr("type").unwrap_or("").trim().to_ascii_lowercase();
    let control = node.child("control");
    let control_type = control
        .map(|item| item.attr("type").unwrap_or("").trim().to_ascii_lowercase())
        .unwrap_or_default();
    let control_format = control
        .map(|item| {
            item.attr("format")
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let kind = kind_of(&setting_type, &control_type, &control_format, node);
    let label = localize(&label_raw(node, control), strings);
    let mut id = node
        .attr("id")
        .map(|text| text.to_string())
        .filter(|text| !text.trim().is_empty());
    if kind == RawKind::Header {
        id = None;
    }
    let (visible_expr, enable_expr) = visibility(node);
    let options = options_of(node, setting_type == "enum", strings);
    RawField {
        id,
        label: label.clone(),
        kind,
        default_value: default_of(node),
        secret: is_secret(node, &label, control),
        options,
        visible_expr,
        enable_expr,
        action: if kind == RawKind::Action || node.attr("action").is_some() {
            action_of(node)
        } else {
            String::new()
        },
        eq_slot: true,
    }
}

fn kind_of(
    setting_type: &str,
    control_type: &str,
    control_format: &str,
    node: &XmlNode,
) -> RawKind {
    const HEADERS: &[&str] = &["lsep", "sep", "separator"];
    if HEADERS.contains(&setting_type) || HEADERS.contains(&control_type) {
        return RawKind::Header;
    }
    if control_type == "button" || setting_type == "action" || setting_type == "button" {
        return RawKind::Action;
    }
    if control_type == "slider" || setting_type == "slider" {
        return RawKind::Number;
    }
    if control_type == "toggle" || setting_type == "bool" || setting_type == "boolean" {
        return RawKind::Bool;
    }
    if control_type == "list" || matches!(setting_type, "enum" | "labelenum" | "select" | "list") {
        return RawKind::Select;
    }
    if control_type == "spinner" {
        if has_options(node) || matches!(setting_type, "enum" | "labelenum" | "select" | "list") {
            return RawKind::Select;
        }
        return RawKind::Number;
    }
    if matches!(setting_type, "number" | "integer")
        || matches!(control_format, "integer" | "number")
    {
        return RawKind::Number;
    }
    if has_options(node) {
        return RawKind::Select;
    }
    RawKind::Text
}

fn label_raw(node: &XmlNode, control: Option<&XmlNode>) -> String {
    if let Some(label) = node.attr("label") {
        return label.to_string();
    }
    if let Some(control) = control {
        if let Some(heading) = control.child("heading") {
            let text = heading.text.trim();
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    String::new()
}

fn default_of(node: &XmlNode) -> String {
    if let Some(value) = node.attr("default") {
        return value.to_string();
    }
    node.child("default")
        .map(|child| child.text.trim().to_string())
        .unwrap_or_default()
}

/// `indexed` is a legacy `enum`. Kodi saves its choice as the option's
/// position, or as its number in `entries`, never as the label.
fn options_of(node: &XmlNode, indexed: bool, strings: &BTreeMap<i64, String>) -> Vec<SelectOption> {
    let listed = node
        .attr("values")
        .filter(|text| !text.is_empty())
        .map(|values| (values, false))
        .or_else(|| {
            node.attr("lvalues")
                .filter(|text| !text.is_empty())
                .map(|values| (values, true))
        });
    if let Some((values, localized)) = listed {
        let entries: Vec<&str> = node
            .attr("entries")
            .map(|text| text.split('|').map(str::trim).collect())
            .unwrap_or_default();
        return values
            .split('|')
            .enumerate()
            .map(|(index, part)| {
                let label = if localized {
                    localize(part, strings)
                } else {
                    plain_label(part)
                };
                let value = if !indexed {
                    part.to_string()
                } else if let Some(entry) = entries.get(index).filter(|entry| !entry.is_empty()) {
                    entry.to_string()
                } else {
                    index.to_string()
                };
                SelectOption { value, label }
            })
            .collect();
    }
    let mut options = Vec::new();
    collect_options(node, strings, &mut options);
    options
}

fn collect_options(node: &XmlNode, strings: &BTreeMap<i64, String>, out: &mut Vec<SelectOption>) {
    for child in &node.children {
        if child.name == "option" {
            let text = child.text.trim();
            if !text.is_empty() {
                // `<option label="32001">2</option>` saves 2 and shows the label.
                let label = child
                    .attr("label")
                    .filter(|label| !label.trim().is_empty())
                    .map(|label| localize(label, strings))
                    .unwrap_or_else(|| text.to_string());
                out.push(SelectOption {
                    value: text.to_string(),
                    label,
                });
            } else if let Some(label) = child.attr("label") {
                let shown = localize(label, strings);
                out.push(SelectOption {
                    value: shown.clone(),
                    label: shown,
                });
            }
        } else {
            collect_options(child, strings, out);
        }
    }
}

fn has_options(node: &XmlNode) -> bool {
    node.attr("values").is_some_and(|text| !text.is_empty())
        || node.attr("lvalues").is_some_and(|text| !text.is_empty())
        || has_named(node, "option")
}

fn has_named(node: &XmlNode, name: &str) -> bool {
    node.children
        .iter()
        .any(|child| child.name == name || has_named(child, name))
}

fn is_secret(node: &XmlNode, label: &str, control: Option<&XmlNode>) -> bool {
    if let Some(option) = node.attr("option") {
        if option
            .split(|ch: char| ch == '|' || ch == ',' || ch.is_whitespace())
            .any(|part| part.eq_ignore_ascii_case("hidden"))
        {
            return true;
        }
    }
    if let Some(control) = control {
        if hidden_true(control) {
            return true;
        }
    }
    secret_label(label)
}

fn hidden_true(node: &XmlNode) -> bool {
    if node.name == "hidden" && node.text.trim().eq_ignore_ascii_case("true") {
        return true;
    }
    node.children.iter().any(hidden_true)
}

fn secret_label(label: &str) -> bool {
    let lower = format!(" {label} ").to_ascii_lowercase();
    [
        " password ",
        " secret ",
        " token ",
        " apikey ",
        " api key ",
        " pin ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || lower.contains("api key")
}

fn visibility(node: &XmlNode) -> (String, String) {
    let visible = node
        .attr("visible")
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| direct_text(node, "visible"));
    let enable = node
        .attr("enable")
        .or_else(|| node.attr("enabled"))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| direct_text(node, "enable"));
    let (dep_visible, dep_enable) = dependencies(node);
    (
        and_expr(&visible, &dep_visible),
        and_expr(&enable, &dep_enable),
    )
}

fn dependencies(node: &XmlNode) -> (String, String) {
    let Some(block) = node.child("dependencies") else {
        return (String::new(), String::new());
    };
    let mut visible = Vec::new();
    let mut enable = Vec::new();
    for dep in &block.children {
        if dep.name != "dependency" {
            continue;
        }
        let kind = dep
            .attr("type")
            .unwrap_or("enable")
            .trim()
            .to_ascii_lowercase();
        let clauses = dependency_clauses(dep);
        if kind == "visible" {
            visible.extend(clauses);
        } else {
            enable.extend(clauses);
        }
    }
    (and_all(&visible), and_all(&enable))
}

fn dependency_clauses(dep: &XmlNode) -> Vec<String> {
    let mut clauses = Vec::new();
    if dep.attr("setting").is_some() || dep.attr("operator").is_some() {
        let clause = condition_clause(dep);
        if !clause.is_empty() {
            clauses.push(clause);
        }
    }
    for child in &dep.children {
        if child.name == "condition" {
            let clause = condition_clause(child);
            if !clause.is_empty() {
                clauses.push(clause);
            }
        }
    }
    if !clauses.is_empty() {
        return clauses;
    }
    let text = dep.text.trim();
    if text.is_empty() {
        Vec::new()
    } else {
        vec![text.to_string()]
    }
}

fn condition_clause(node: &XmlNode) -> String {
    if node.attr("on") == Some("property") {
        return node.text.trim().to_string();
    }
    let id = node.attr("setting").unwrap_or("").trim();
    let operator = node
        .attr("operator")
        .unwrap_or("is")
        .trim()
        .to_ascii_lowercase();
    let value = node.text.trim();
    if id.is_empty() {
        return value.to_string();
    }
    if operator == "is" {
        format!("is({id},{value})")
    } else if operator == "!is" {
        format!("!is({id},{value})")
    } else {
        format!("{operator}({id},{value})")
    }
}

fn and_expr(left: &str, right: &str) -> String {
    and_all(&[left.to_string(), right.to_string()])
}

fn and_all(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("+")
}

fn direct_text(node: &XmlNode, name: &str) -> String {
    node.child(name)
        .map(|child| child.text.trim().to_string())
        .unwrap_or_default()
}

fn action_of(node: &XmlNode) -> String {
    if let Some(action) = node.attr("action") {
        return action.to_string();
    }
    if let Some(data) = node.child("data") {
        let text = data.text.trim();
        if !text.is_empty() {
            return text.to_string();
        }
    }
    fn find_data(node: &XmlNode) -> Option<String> {
        for child in &node.children {
            if child.name == "data" {
                let text = child.text.trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
            if let Some(found) = find_data(child) {
                return Some(found);
            }
        }
        None
    }
    find_data(node).unwrap_or_default()
}

fn localize(text: &str, strings: &BTreeMap<i64, String>) -> String {
    let raw = text.trim();
    let resolved = if let Some(id) = string_id(raw) {
        strings
            .get(&id)
            .cloned()
            .unwrap_or_else(|| text.to_string())
    } else {
        text.to_string()
    };
    plain_label(&expand_setting_tokens(&resolved, strings))
}

/// `$NUMBER[1337]` is the number itself. `$LOCALIZE[id]` and
/// `$ADDON[addon.id id]` use this add-on's strings when that id is known.
fn expand_setting_tokens(text: &str, strings: &BTreeMap<i64, String>) -> String {
    let mut current = text.to_string();
    for _ in 0..3 {
        let next = expand_setting_tokens_once(&current, strings);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn expand_setting_tokens_once(text: &str, strings: &BTreeMap<i64, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some((name, inner, consumed)) = setting_token(after) {
            if let Some(value) = token_value(name, inner, strings) {
                out.push_str(&value);
                rest = &after[consumed..];
                continue;
            }
        }
        out.push('$');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn setting_token(after: &str) -> Option<(&str, &str, usize)> {
    let bytes = after.as_bytes();
    let mut name_len = 0;
    while name_len < bytes.len() && bytes[name_len].is_ascii_alphabetic() {
        name_len += 1;
    }
    if name_len == 0 || name_len >= bytes.len() || bytes[name_len] != b'[' {
        return None;
    }
    let name = &after[..name_len];
    if !matches!(
        name.to_ascii_lowercase().as_str(),
        "number" | "localize" | "addon"
    ) {
        return None;
    }
    let inner_start = name_len + 1;
    let inner_rel = after[inner_start..].find(']')?;
    let inner = &after[inner_start..inner_start + inner_rel];
    Some((name, inner, inner_start + inner_rel + 1))
}

fn token_value(name: &str, inner: &str, strings: &BTreeMap<i64, String>) -> Option<String> {
    match name.to_ascii_lowercase().as_str() {
        "number" => Some(inner.trim().to_string()),
        "localize" => Some(string_text(inner.trim(), strings)),
        "addon" => {
            let id = inner.split_whitespace().next_back()?.trim();
            Some(string_text(id, strings))
        }
        _ => None,
    }
}

fn string_text(text: &str, strings: &BTreeMap<i64, String>) -> String {
    match string_id(text).and_then(|id| strings.get(&id)) {
        Some(value) => value.clone(),
        None => text.to_string(),
    }
}

fn string_id(text: &str) -> Option<i64> {
    if !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit()) {
        return text.parse().ok();
    }
    let lower = text.to_ascii_lowercase();
    let inner = if let Some(rest) = lower.strip_prefix("$localize[") {
        rest.strip_suffix(']')?
    } else if let Some(rest) = lower.strip_prefix("$addon[") {
        rest.strip_suffix(']')?
    } else {
        text.strip_prefix('#')?
    };
    if inner.chars().all(|ch| ch.is_ascii_digit()) && !inner.is_empty() {
        inner.parse().ok()
    } else {
        None
    }
}

fn load_strings(addon_dir: &Path) -> BTreeMap<i64, String> {
    let candidates = [
        addon_dir
            .join("resources")
            .join("language")
            .join("resource.language.en_gb")
            .join("strings.po"),
        addon_dir
            .join("resources")
            .join("language")
            .join("English")
            .join("strings.po"),
        addon_dir
            .join("resources")
            .join("language")
            .join("English")
            .join("strings.xml"),
    ];
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            return BTreeMap::new();
        };
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .eq_ignore_ascii_case("po")
        {
            return parse_po(&text);
        }
        return parse_strings_xml(&text);
    }
    BTreeMap::new()
}

fn parse_po(text: &str) -> BTreeMap<i64, String> {
    let mut found = BTreeMap::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut index = 0;
    let mut pending: Option<i64> = None;
    while index < lines.len() {
        let line = lines[index].trim();
        if line.starts_with("msgctxt") {
            pending = po_context_id(line);
            index += 1;
            continue;
        }
        if line.starts_with("msgid") {
            if let Some(id) = pending.take() {
                let (value, next) = read_po_string(&lines, index);
                found.insert(id, value);
                index = next;
                continue;
            }
        }
        index += 1;
    }
    found
}

fn po_context_id(line: &str) -> Option<i64> {
    let hash = line.rfind('#')?;
    let digits: String = line[hash + 1..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn read_po_string(lines: &[&str], index: usize) -> (String, usize) {
    let first = lines[index].trim();
    let body = first.trim_start_matches("msgid").trim();
    let mut parts = Vec::new();
    if !body.is_empty() {
        parts.push(po_unquote(body));
    }
    let mut index = index + 1;
    while index < lines.len() {
        let next = lines[index].trim();
        if next.len() >= 2 && next.starts_with('"') && next.ends_with('"') {
            parts.push(po_unquote(next));
            index += 1;
            continue;
        }
        break;
    }
    (parts.concat(), index)
}

fn po_unquote(token: &str) -> String {
    let token = token.trim().trim_matches('"');
    let mut out = String::new();
    let mut chars = token.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_strings_xml(text: &str) -> BTreeMap<i64, String> {
    let mut found = BTreeMap::new();
    let Ok(root) = parse_xml(text) else {
        return found;
    };
    let mut strings = Vec::new();
    collect_all(&root, "string", &mut strings);
    for node in strings {
        if let Some(id) = node.attr("id").and_then(|raw| raw.parse::<i64>().ok()) {
            found.insert(id, node.text.clone());
        }
    }
    found
}

fn collect_all<'a>(node: &'a XmlNode, name: &str, out: &mut Vec<&'a XmlNode>) {
    if node.name == name {
        out.push(node);
    }
    for child in &node.children {
        collect_all(child, name, out);
    }
}

fn read_saved(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(root) = parse_xml(&text) else {
        return BTreeMap::new();
    };
    if root.name != "settings" {
        return BTreeMap::new();
    }
    let version = root.attr("version").unwrap_or("").trim().to_string();
    let mut found = BTreeMap::new();
    let mut settings = Vec::new();
    collect_all(&root, "setting", &mut settings);
    for node in settings {
        let Some(id) = node.attr("id").filter(|text| !text.is_empty()) else {
            continue;
        };
        let value = if let Some(value) = node.attr("value") {
            value.to_string()
        } else if version == "2" {
            node.text.trim().to_string()
        } else if let Some(value) = node.attr("default") {
            value.to_string()
        } else {
            let text = node.text.trim();
            if text.is_empty() {
                continue;
            }
            text.to_string()
        };
        found.insert(id.to_string(), value);
    }
    found
}

fn write_saved(path: &Path, setting_id: &str, value: &str) -> Result<(), EngineError> {
    write_saved_many(path, &[(setting_id, value)])
}

fn write_saved_many(path: &Path, pairs: &[(&str, &str)]) -> Result<(), EngineError> {
    if pairs.is_empty() {
        return Err(EngineError::Message(
            "Could not write the add-on settings.".into(),
        ));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|err| EngineError::Message(err.to_string()))?;
    }
    // Read, change and replace as one step, so two saves at once cannot each
    // keep only their own value.
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = WRITING.lock().unwrap_or_else(|err| err.into_inner());
    // Only a missing file starts empty. One that cannot be read now, as while
    // Kodi holds it or when it is not UTF-8, would be replaced by a file with
    // just these values, and every other saved setting would be lost.
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            return Err(EngineError::Message(
                "Could not write the add-on settings.".into(),
            ))
        }
    };
    let mut root = if existing.trim().is_empty() {
        XmlNode {
            name: "settings".into(),
            attrs: vec![("version".into(), "2".into())],
            text: String::new(),
            children: Vec::new(),
        }
    } else if xml_complete(&existing) {
        parse_xml(&existing)
            .map_err(|_| EngineError::Message("Could not write the add-on settings.".into()))?
    } else {
        // A file cut off mid-write still parses leniently, but rewriting it
        // would drop every setting past the cut for good.
        return Err(EngineError::Message(
            "Could not write the add-on settings.".into(),
        ));
    };
    if root.name != "settings" {
        return Err(EngineError::Message(
            "Could not write the add-on settings.".into(),
        ));
    }
    let version = root.attr("version").unwrap_or("").trim().to_string();
    for &(setting_id, value) in pairs {
        if let Some(node) = find_setting_mut(&mut root, setting_id) {
            put_saved_value(node, &version, value);
        } else if version == "2" || version.is_empty() {
            root.children.push(XmlNode {
                name: "setting".into(),
                attrs: vec![("id".into(), setting_id.to_string())],
                text: value.to_string(),
                children: Vec::new(),
            });
        } else {
            root.children.push(XmlNode {
                name: "setting".into(),
                attrs: vec![
                    ("id".into(), setting_id.to_string()),
                    ("value".into(), value.to_string()),
                ],
                text: String::new(),
                children: Vec::new(),
            });
        }
    }
    let mut body = String::new();
    write_node(&root, &mut body);
    let tmp = path.with_extension("xml.tmp");
    fs::write(&tmp, body).map_err(|err| EngineError::Message(err.to_string()))?;
    fs::rename(&tmp, path).map_err(|err| EngineError::Message(err.to_string()))?;
    Ok(())
}

fn find_setting_mut<'a>(node: &'a mut XmlNode, id: &str) -> Option<&'a mut XmlNode> {
    if node.name == "setting" && node.attr("id") == Some(id) {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(found) = find_setting_mut(child, id) {
            return Some(found);
        }
    }
    None
}

fn put_saved_value(node: &mut XmlNode, version: &str, value: &str) {
    if node.attrs.iter().any(|(key, _)| key == "value") {
        for (key, current) in &mut node.attrs {
            if key == "value" {
                *current = value.to_string();
            }
        }
        mark_setting_changed(node);
        return;
    }
    if version == "2" {
        node.text = value.to_string();
        mark_setting_changed(node);
        return;
    }
    node.attrs.push(("value".into(), value.to_string()));
    mark_setting_changed(node);
}

/// Kodi settings v2 keeps `default="true"` until the value changes. Leaving that
/// flag set makes getSetting ignore the written text and keep the empty default.
fn mark_setting_changed(node: &mut XmlNode) {
    if let Some((_, current)) = node.attrs.iter_mut().find(|(key, _)| key == "default") {
        *current = "false".to_string();
    }
}

fn write_node(node: &XmlNode, out: &mut String) {
    out.push('<');
    out.push_str(&node.name);
    for (key, value) in &node.attrs {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        out.push_str(&xml_escape(value));
        out.push('"');
    }
    if node.children.is_empty() && node.text.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    out.push_str(&xml_escape(&node.text));
    for child in &node.children {
        write_node(child, out);
    }
    out.push_str("</");
    out.push_str(&node.name);
    out.push('>');
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

struct XmlNode {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<XmlNode>,
}

impl XmlNode {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|child| child.name == name)
    }
}

fn parse_xml(xml: &str) -> Result<XmlNode, String> {
    let xml = xml.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let owned = event.into_owned();
                buf.clear();
                return read_element(
                    &mut reader,
                    &mut buf,
                    local_name(owned.name().as_ref()),
                    collect_attrs(&owned),
                );
            }
            Ok(Event::Empty(event)) => {
                return Ok(XmlNode {
                    name: local_name(event.name().as_ref()),
                    attrs: collect_attrs(&event),
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Eof) => return Err("empty document".into()),
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }
}

/// Whether the text is one whole XML document, with every element closed.
pub(crate) fn xml_complete(xml: &str) -> bool {
    let mut reader = Reader::from_str(xml.trim_start_matches('\u{feff}'));
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
            }
            Ok(Event::Empty(_)) if depth == 0 => roots += 1,
            Ok(Event::End(_)) => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Ok(Event::Eof) => return depth == 0 && roots == 1,
            Err(_) => return false,
            _ => {}
        }
    }
}

fn read_element(
    reader: &mut Reader<&[u8]>,
    buf: &mut Vec<u8>,
    name: String,
    attrs: Vec<(String, String)>,
) -> Result<XmlNode, String> {
    let mut node = XmlNode {
        name,
        attrs,
        text: String::new(),
        children: Vec::new(),
    };
    loop {
        match reader.read_event_into(buf) {
            Ok(Event::Start(event)) => {
                let owned = event.into_owned();
                buf.clear();
                node.children.push(read_element(
                    reader,
                    buf,
                    local_name(owned.name().as_ref()),
                    collect_attrs(&owned),
                )?);
            }
            Ok(Event::Empty(event)) => node.children.push(XmlNode {
                name: local_name(event.name().as_ref()),
                attrs: collect_attrs(&event),
                text: String::new(),
                children: Vec::new(),
            }),
            Ok(Event::Text(event)) => {
                if let Ok(text) = event.unescape() {
                    node.text.push_str(&text);
                }
            }
            Ok(Event::CData(event)) => node.text.push_str(&String::from_utf8_lossy(&event)),
            Ok(Event::End(_)) | Ok(Event::Eof) => break,
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }
    Ok(node)
}

fn local_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.rsplit('}').next().unwrap_or(text.as_ref());
    text.rsplit(':').next().unwrap_or(text).to_string()
}

fn collect_attrs(event: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    event
        .attributes()
        .filter_map(|attr| attr.ok())
        .map(|attr| {
            let key = local_name(attr.key.as_ref());
            let value = attr
                .unescape_value()
                .map(|text| text.into_owned())
                .unwrap_or_default();
            (key, value)
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/settings.rs"]
mod tests;
