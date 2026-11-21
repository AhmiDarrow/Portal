use super::*;

fn langs(list: &[&str]) -> Vec<String> {
    list.iter().map(|code| code.to_string()).collect()
}

/// A search for an episode as SubDL answers it, trimmed.
const EPISODE: &str = r#"{
  "status": true,
  "results": [{"sd_id": 1001, "type": "tv", "name": "Some Show", "imdb_id": "tt0000001", "year": 2005}],
  "subtitles": [
    {"release_name": "Some.Show.S01E02.720p.WEB.h264-GrP", "name": "SUBDL::1001-3001", "lang": "english",
     "author": "someone", "url": "/subtitle/1001-3001.zip", "subtitlePage": "/s/info/abc/some-show",
     "season": 1, "episode": 2, "language": "EN", "hi": false, "episode_from": 2, "episode_end": 0,
     "full_season": false, "releases": ["Some.Show.S01E02.720p.WEB.h264-GrP", "Some.Show.S01E02.1080p.WEB.h264-GrP"]},
    {"release_name": "Some.Show.Season.1.Complete", "name": "SUBDL::1001-3002", "lang": "Brazillian Portuguese",
     "author": "x", "url": "/subtitle/1001-3002.zip", "season": 1, "episode": 0, "language": "BR_PT",
     "hi": true, "full_season": true, "releases": []},
    {"release_name": "Some.Show.S01E03.720p", "name": "SUBDL::1001-3003", "lang": "english",
     "url": "/subtitle/1001-3003.zip", "season": 1, "episode": 3, "language": "EN", "full_season": false},
    {"release_name": "Some.Show.S02.Complete", "name": "SUBDL::1001-3004", "lang": "english",
     "url": "/subtitle/1001-3004.zip", "season": 2, "episode": null, "language": "EN", "full_season": true},
    {"release_name": "Some Show", "name": "SUBDL::1001-3005", "lang": "english",
     "url": "/subtitle/1001-3005.zip", "season": null, "episode": null, "language": "EN"},
    {"release_name": "Evil", "lang": "english", "url": "https://evil.example/x.zip", "language": "EN"},
    {"release_name": "Klingon", "lang": "Klingon", "url": "/subtitle/1001-3006.zip", "language": "TLH"}
  ],
  "totalPages": 1
}"#;

#[test]
fn episode_rows_are_read_and_filtered() {
    let json: Value = serde_json::from_str(EPISODE).unwrap();
    let search = Search {
        movie: false,
        imdb: Some("tt0000001".into()),
        episode: Some((1, 2)),
        ..Search::default()
    };
    let found = answer(StatusCode::OK, Some(&json), &search).unwrap();
    let ids: Vec<&str> = found.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["subdl:1001-3001", "subdl:1001-3002", "subdl:1001-3005"]
    );
    assert_eq!(found[0].language, "en");
    assert_eq!(
        found[0].release,
        "Some.Show.S01E02.720p.WEB.h264-GrP\nSome.Show.S01E02.1080p.WEB.h264-GrP"
    );
    assert!(!found[0].episode_unsure);
    assert_eq!(found[1].language, "pt-BR");
    assert!(found[1].hearing_impaired);
    assert_eq!(found[1].episode, Some((1, 2)));
    assert!(!found[1].episode_unsure);
    assert!(found[2].episode_unsure);
}

#[test]
fn movie_rows_keep_everything_with_a_file() {
    let json = serde_json::json!({"status": true, "subtitles": [
        {"release_name": "Movie.1994.1080p.BluRay.x264-GRPK", "lang": "French", "language": "fr",
         "url": "/subtitle/77-88.zip", "season": null, "episode": null, "hi": false}
    ]});
    let search = Search {
        movie: true,
        imdb: Some("tt0000004".into()),
        ..Search::default()
    };
    let found = parse(&json, &search);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].language, "fr");
    assert_eq!(found[0].episode, None);
    assert!(!found[0].episode_unsure);
}

#[test]
fn refused_keys_and_empty_answers_differ() {
    let search = Search::default();
    let bad = serde_json::json!({"status": false, "error": "API_KEY is not valid"});
    assert_eq!(
        answer(StatusCode::OK, Some(&bad), &search),
        Err(SourceError::Note(SAVED_KEY_REFUSED.into()))
    );
    let bare = serde_json::json!({"status": false});
    assert_eq!(answer(StatusCode::OK, Some(&bare), &search), Ok(Vec::new()));
    let none = serde_json::json!({"status": false, "error": "can't find movie or tv"});
    assert_eq!(answer(StatusCode::OK, Some(&none), &search), Ok(Vec::new()));
    assert_eq!(
        answer(StatusCode::FORBIDDEN, None, &search),
        Err(SourceError::Note(SAVED_KEY_REFUSED.into()))
    );
    assert_eq!(
        answer(StatusCode::TOO_MANY_REQUESTS, None, &search),
        Err(SourceError::Note(BUSY.into()))
    );
    assert_eq!(
        answer(StatusCode::BAD_GATEWAY, None, &search),
        Err(SourceError::Failed)
    );
}

#[test]
fn params_prefer_the_imdb_id_and_map_languages() {
    let episode = Search {
        movie: false,
        imdb: Some("tt0000001".into()),
        name: Some("Some Show".into()),
        episode: Some((1, 2)),
    };
    let pairs = params(&episode, &langs(&["pt-BR", "en", "eu", "pt-PT"])).unwrap();
    assert_eq!(
        pairs,
        vec![
            ("type", "tv".to_string()),
            ("imdb_id", "tt0000001".to_string()),
            ("season_number", "1".to_string()),
            ("episode_number", "2".to_string()),
            ("languages", "BR_PT,EN,PT".to_string()),
            ("subs_per_page", "30".to_string()),
            ("releases", "1".to_string()),
        ]
    );
    let by_name = Search {
        movie: true,
        name: Some("Storm".into()),
        episode: Some((1, 1)),
        ..Search::default()
    };
    let pairs = params(&by_name, &langs(&["fr"])).unwrap();
    assert_eq!(pairs[0], ("type", "movie".to_string()));
    assert_eq!(pairs[1], ("film_name", "Storm".to_string()));
    assert_eq!(pairs[2], ("languages", "FR".to_string()));
    assert_eq!(params(&by_name, &langs(&["eu"])), None);
    assert_eq!(params(&Search::default(), &langs(&["en"])), None);
}

#[test]
fn only_subdl_zip_paths_become_ids() {
    assert_eq!(path_id("/subtitle/1234-5678.zip"), Some("1234-5678"));
    assert_eq!(path_id("/subtitle/a_B-9.zip"), Some("a_B-9"));
    for bad in [
        "/subtitle/../x.zip",
        "/subtitle/a/b.zip",
        "/subtitle/.zip",
        "/subtitle/1234.rar",
        "https://dl.subdl.com/subtitle/1.zip",
        "/other/1.zip",
        "/subtitle/1 2.zip",
    ] {
        assert_eq!(path_id(bad), None, "{bad}");
    }
    assert!(valid_id("1234-5678"));
    assert!(!valid_id("1234/5678"));
}
