use super::*;

fn langs(list: &[&str]) -> Vec<String> {
    list.iter().map(|code| code.to_string()).collect()
}

#[test]
fn imdb_ids_lose_tt_and_leading_zeros() {
    assert_eq!(imdb_digits("tt0000004").as_deref(), Some("4"));
    assert_eq!(
        imdb_digits("https://www.imdb.com/title/tt0000001/").as_deref(),
        Some("1")
    );
    assert_eq!(imdb_digits("0000004"), None);
}

#[test]
fn params_are_sorted_and_prefer_ids() {
    let episode = Search {
        movie: false,
        parent_imdb: Some("1".into()),
        query: Some("Some Show".into()),
        season: Some(1),
        episode: Some(2),
        ..Search::default()
    };
    let pairs = params(&episode, &langs(&["pt-BR", "en"]));
    let names: Vec<&str> = pairs.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        vec![
            "episode_number",
            "languages",
            "parent_imdb_id",
            "season_number",
            "type"
        ]
    );
    assert_eq!(pairs[1].1, "en,pt-br");
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);

    let movie = Search {
        movie: true,
        query: Some("Storm".into()),
        year: Some(1995),
        season: Some(1),
        ..Search::default()
    };
    let pairs = params(&movie, &langs(&["en"]));
    assert_eq!(
        pairs,
        vec![
            ("languages", "en".to_string()),
            ("query", "storm".to_string()),
            ("type", "movie".to_string()),
            ("year", "1995".to_string()),
        ]
    );
}

#[test]
fn rows_are_read_and_filtered() {
    let json = serde_json::json!({"data": [
        {"attributes": {"language": "en", "download_count": 201, "hearing_impaired": false, "ratings": 0.0,
         "release": "Show.S01E02.720p.WEB-GrP", "moviehash_match": true,
         "feature_details": {"season_number": 1, "episode_number": 2},
         "files": [{"file_id": 12658811, "file_name": "x.en"}]}},
        {"attributes": {"language": "pt-BR", "download_count": 3, "hearing_impaired": true, "ratings": 6.5,
         "release": "Other", "feature_details": {"season_number": 1, "episode_number": 2},
         "files": [{"file_id": 5}]}},
        {"attributes": {"language": "en", "release": "Wrong episode",
         "feature_details": {"season_number": 1, "episode_number": 3}, "files": [{"file_id": 6}]}},
        {"attributes": {"language": "en", "machine_translated": true, "files": [{"file_id": 7}]}},
        {"attributes": {"language": "en", "files": []}}
    ]});
    let search = Search {
        movie: false,
        season: Some(1),
        episode: Some(2),
        ..Search::default()
    };
    let found = parse(&json, &search);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, "opensubtitles:12658811");
    assert!(found[0].hash_match);
    assert_eq!(found[0].rating, None);
    assert_eq!(found[1].language, "pt-BR");
    assert_eq!(found[1].rating, Some(6.5));
    assert!(found[1].hearing_impaired);
}

#[test]
fn only_opensubtitles_hosts_are_download_roots() {
    assert_eq!(
        api_base("vip-api.opensubtitles.com").as_deref(),
        Some("https://vip-api.opensubtitles.com/api/v1")
    );
    assert_eq!(
        api_base("api.opensubtitles.com").as_deref(),
        Some("https://api.opensubtitles.com/api/v1")
    );
    assert_eq!(api_base("evil.example"), None);
    assert_eq!(api_base("opensubtitles.com.evil.example"), None);
    assert!(valid_id("12658811"));
    assert!(!valid_id("12a"));
}

#[test]
fn quota_and_key_errors_explain_themselves() {
    assert_eq!(
        refusal(StatusCode::NOT_ACCEPTABLE, KEY_REFUSED),
        SourceError::Note(QUOTA.into())
    );
    assert_eq!(
        refusal(StatusCode::FORBIDDEN, KEY_REFUSED),
        SourceError::Note(KEY_REFUSED.into())
    );
    assert_eq!(
        refusal(StatusCode::INTERNAL_SERVER_ERROR, KEY_REFUSED),
        SourceError::Failed
    );
    // Signing in: 401 is the account, 403 the key.
    assert_eq!(
        refusal(StatusCode::UNAUTHORIZED, LOGIN_REFUSED),
        SourceError::Note(LOGIN_REFUSED.into())
    );
    assert_eq!(
        refusal(StatusCode::FORBIDDEN, LOGIN_REFUSED),
        SourceError::Note(SAVED_KEY_REFUSED.into())
    );
    let session = Session {
        token: "SECRET".into(),
        base: API.into(),
    };
    assert!(!format!("{session:?}").contains("SECRET"));
}
