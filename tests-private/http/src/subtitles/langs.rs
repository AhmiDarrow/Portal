use super::*;

#[test]
fn languages_are_found_by_code_name_or_alias() {
    assert_eq!(find("en").unwrap().code, "en");
    assert_eq!(find("EN-us").unwrap().code, "en");
    assert_eq!(find("pt_BR").unwrap().code, "pt-BR");
    assert_eq!(find("pt").unwrap().code, "pt-PT");
    assert_eq!(find("Brazilian Portuguese").unwrap().code, "pt-BR");
    assert_eq!(find("Farsi/Persian").unwrap().code, "fa");
    assert_eq!(find("Big 5 code").unwrap().code, "zh-TW");
    assert_eq!(find("zh-HK").unwrap().code, "zh-TW");
    assert_eq!(find("zh").unwrap().code, "zh-CN");
    assert_eq!(find("nb-NO").unwrap().code, "no");
    assert!(find("klingon").is_none());
    assert!(find("").is_none());
}

#[test]
fn lists_are_normalized_and_bounded() {
    let list: Vec<String> = ["en-GB", "English", "fr", "xx", "de", "es", "it"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(normalize(&list), vec!["en", "fr", "de", "es"]);
    assert_eq!(name_of("pt-BR"), "Portuguese (Brazil)");
    assert!(same_family("pt-BR", "pt-PT"));
    assert!(!same_family("en", "es"));
}

#[test]
fn legacy_encodings_follow_the_language() {
    assert_eq!(legacy_encoding(Some("ru")), encoding_rs::WINDOWS_1251);
    assert_eq!(legacy_encoding(Some("ar")), encoding_rs::WINDOWS_1256);
    assert_eq!(legacy_encoding(Some("zh-TW")), encoding_rs::BIG5);
    assert_eq!(legacy_encoding(None), encoding_rs::WINDOWS_1252);
    for language in LANGUAGES {
        assert!(
            Encoding::for_label(language.legacy.as_bytes()).is_some(),
            "{}",
            language.code
        );
    }
}
