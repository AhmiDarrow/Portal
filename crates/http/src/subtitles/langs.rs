//! The languages Portal can ask subtitle sources for: a code (as sent to the
//! sources and the page), an English name, the other names sources use for
//! it, and the legacy Windows code page its old subtitle files tend to use.

use encoding_rs::Encoding;

pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
    /// Other names a source may print, lowercase.
    aliases: &'static [&'static str],
    /// The usual pre-Unicode encoding, by WHATWG label.
    legacy: &'static str,
}

const fn lang(
    code: &'static str,
    name: &'static str,
    aliases: &'static [&'static str],
    legacy: &'static str,
) -> Language {
    Language {
        code,
        name,
        aliases,
        legacy,
    }
}

pub const LANGUAGES: &[Language] = &[
    lang("en", "English", &["eng"], "windows-1252"),
    lang(
        "es",
        "Spanish",
        &[
            "spa",
            "spanish (spain)",
            "spanish (latin america)",
            "espanol",
        ],
        "windows-1252",
    ),
    lang("fr", "French", &["fre", "fra", "francais"], "windows-1252"),
    lang("de", "German", &["ger", "deu", "deutsch"], "windows-1252"),
    lang("it", "Italian", &["ita"], "windows-1252"),
    lang(
        "pt-PT",
        "Portuguese",
        &["pt", "por", "portuguese (portugal)"],
        "windows-1252",
    ),
    lang(
        "pt-BR",
        "Portuguese (Brazil)",
        &[
            "pob",
            "brazilian portuguese",
            "portuguese-br",
            "portuguese (brazilian)",
        ],
        "windows-1252",
    ),
    lang("nl", "Dutch", &["dut", "nld"], "windows-1252"),
    lang("sv", "Swedish", &["swe"], "windows-1252"),
    lang("da", "Danish", &["dan"], "windows-1252"),
    lang("no", "Norwegian", &["nb", "nn", "nor"], "windows-1252"),
    lang("fi", "Finnish", &["fin"], "windows-1252"),
    lang("is", "Icelandic", &["ice", "isl"], "windows-1252"),
    lang("ca", "Catalan", &["cat"], "windows-1252"),
    lang("eu", "Basque", &["baq", "eus"], "windows-1252"),
    lang("gl", "Galician", &["glg"], "windows-1252"),
    lang("id", "Indonesian", &["ind"], "windows-1252"),
    lang("ms", "Malay", &["may", "msa"], "windows-1252"),
    lang("pl", "Polish", &["pol"], "windows-1250"),
    lang("cs", "Czech", &["cze", "ces"], "windows-1250"),
    lang("sk", "Slovak", &["slo", "slk"], "windows-1250"),
    lang("hu", "Hungarian", &["hun"], "windows-1250"),
    lang("ro", "Romanian", &["rum", "ron"], "windows-1250"),
    lang("hr", "Croatian", &["hrv"], "windows-1250"),
    lang("sl", "Slovenian", &["slv"], "windows-1250"),
    lang("bs", "Bosnian", &["bos"], "windows-1250"),
    lang("sq", "Albanian", &["alb", "sqi"], "windows-1250"),
    lang("sr", "Serbian", &["srp", "scc"], "windows-1250"),
    lang("ru", "Russian", &["rus"], "windows-1251"),
    lang("uk", "Ukrainian", &["ukr"], "windows-1251"),
    lang("bg", "Bulgarian", &["bul"], "windows-1251"),
    lang("mk", "Macedonian", &["mac", "mkd"], "windows-1251"),
    lang("el", "Greek", &["gre", "ell"], "windows-1253"),
    lang("tr", "Turkish", &["tur"], "windows-1254"),
    lang("he", "Hebrew", &["heb"], "windows-1255"),
    lang("ar", "Arabic", &["ara"], "windows-1256"),
    lang(
        "fa",
        "Persian",
        &["per", "fas", "farsi", "farsi/persian"],
        "windows-1256",
    ),
    lang("ur", "Urdu", &["urd"], "windows-1256"),
    lang("et", "Estonian", &["est"], "windows-1257"),
    lang("lv", "Latvian", &["lav"], "windows-1257"),
    lang("lt", "Lithuanian", &["lit"], "windows-1257"),
    lang("vi", "Vietnamese", &["vie"], "windows-1258"),
    lang("th", "Thai", &["tha"], "windows-874"),
    lang("hi", "Hindi", &["hin"], "utf-8"),
    lang("bn", "Bengali", &["ben"], "utf-8"),
    lang("ta", "Tamil", &["tam"], "utf-8"),
    lang("te", "Telugu", &["tel"], "utf-8"),
    lang("ml", "Malayalam", &["mal"], "utf-8"),
    lang("si", "Sinhala", &["sin", "sinhalese"], "utf-8"),
    lang(
        "zh-CN",
        "Chinese (Simplified)",
        &[
            "zh",
            "chi",
            "zho",
            "chinese",
            "zh-hans",
            "chinese simplified",
        ],
        "gbk",
    ),
    lang(
        "zh-TW",
        "Chinese (Traditional)",
        &[
            "zht",
            "zh-hant",
            "big 5 code",
            "chinese traditional",
            "chinese bg code",
        ],
        "big5",
    ),
    lang("ja", "Japanese", &["jpn"], "shift_jis"),
    lang("ko", "Korean", &["kor"], "euc-kr"),
];

/// The most preferred languages one search asks for.
pub const MAX_LANGUAGES: usize = 4;

/// A language by code or name, in any case: "EN", "en-US", "pt_br",
/// "Brazilian Portuguese", "Farsi/Persian". Region subtags other than
/// Portuguese's and Chinese's are dropped.
pub fn find(text: &str) -> Option<&'static Language> {
    let text = text.trim().replace('_', "-").to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    let exact = LANGUAGES.iter().find(|language| {
        language.code.eq_ignore_ascii_case(&text)
            || language.name.eq_ignore_ascii_case(&text)
            || language.aliases.contains(&text.as_str())
    });
    if exact.is_some() {
        return exact;
    }
    let (primary, region) = text.split_once('-')?;
    match (primary, region) {
        ("pt", "br") => find("pt-BR"),
        ("zh", "tw" | "hk" | "mo" | "hant") => find("zh-TW"),
        ("zh", _) => find("zh-CN"),
        _ => find(primary),
    }
}

/// The saved or asked-for languages as known codes, without repeats, at
/// most `MAX_LANGUAGES`.
pub fn normalize(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in list {
        let Some(language) = find(text) else { continue };
        if !out.iter().any(|code| code == language.code) {
            out.push(language.code.to_string());
        }
        if out.len() == MAX_LANGUAGES {
            break;
        }
    }
    out
}

pub fn name_of(code: &str) -> String {
    find(code).map_or_else(|| code.to_string(), |language| language.name.to_string())
}

/// The encoding an old subtitle file in this language most likely uses.
pub fn legacy_encoding(code: Option<&str>) -> &'static Encoding {
    code.and_then(find)
        .and_then(|language| Encoding::for_label(language.legacy.as_bytes()))
        .unwrap_or(encoding_rs::WINDOWS_1252)
}

/// The two codes share a language but not a variant, e.g. pt-BR and pt-PT.
pub fn same_family(a: &str, b: &str) -> bool {
    let primary = |code: &str| code.split('-').next().unwrap_or("").to_ascii_lowercase();
    primary(a) == primary(b)
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/langs.rs"]
mod tests;
