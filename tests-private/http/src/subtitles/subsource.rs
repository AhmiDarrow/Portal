use super::*;

fn langs(list: &[&str]) -> Vec<String> {
    list.iter().map(|code| code.to_string()).collect()
}

/// A show's title lookup as SubSource answers it, trimmed.
const TITLES: &str = r#"{
  "success": true,
  "data": [
    {"movieId": 4100, "title": "Some Show - First Season", "type": "tvseries", "releaseYear": 2005,
     "imdbId": "tt0000001", "season": 1, "subtitleCount": 120, "link": "/subtitles/some-show-first-season"},
    {"movieId": 4101, "title": "Some Show - Second Season", "type": "tvseries", "releaseYear": 2006,
     "imdbId": "tt0000001", "season": 2, "subtitleCount": 80},
    {"movieId": 9, "title": "Other Show", "type": "tvseries", "imdbId": "tt999", "season": 2}
  ]
}"#;

/// A season's subtitle list as SubSource answers it, trimmed.
const SUBTITLES: &str = r#"{
  "success": true,
  "data": [
    {"subtitleId": 9001, "movieId": 4100, "language": "english",
     "releaseInfo": ["Some.Show.S01E02.720p.WEB.h264-GrP", "Some.Show.S01E02.1080p.WEB.h264-GrP"],
     "hearingImpaired": false, "foreignParts": false, "downloads": 1520,
     "rating": {"good": 4, "bad": 0, "total": 4}, "uploader": {"displayname": "x"}},
    {"subtitleId": 9002, "movieId": 4100, "language": "english", "releaseInfo": ["some show 1x03 hdtv-lol"],
     "hearingImpaired": false, "foreignParts": false, "downloads": 900},
    {"subtitleId": 9003, "movieId": 4100, "language": "english", "releaseInfo": ["Some.Show.S01.Complete.BluRay"],
     "hearingImpaired": true, "foreignParts": false, "downloads": 40},
    {"subtitleId": 9004, "movieId": 4100, "language": "english", "releaseInfo": ["Some.Show.S01E02.WEB"],
     "hearingImpaired": false, "foreignParts": true},
    {"subtitleId": 9005, "movieId": 4100, "language": "Brazillian Portuguese",
     "releaseInfo": ["Some.Show.s01e02.WEB", "Some.Show.S01E03.WEB"], "downloads": 3},
    {"movieId": 4100, "language": "english", "releaseInfo": ["No id"]}
  ],
  "pagination": {"page": 1, "limit": 50, "total": 6}
}"#;

fn episode() -> Search {
    Search {
        movie: false,
        imdb: Some("tt0000001".into()),
        episode: Some((1, 2)),
        ..Search::default()
    }
}

#[test]
fn the_season_entry_is_picked() {
    let json: Value = serde_json::from_str(TITLES).unwrap();
    assert_eq!(pick_title(&json, &episode()), Some(4100));
    let second = Search {
        episode: Some((2, 1)),
        ..episode()
    };
    assert_eq!(pick_title(&json, &second), Some(4101));
    let missing = Search {
        episode: Some((5, 1)),
        ..episode()
    };
    assert_eq!(pick_title(&json, &missing), None);

    let movies = serde_json::json!({"success": true, "data": [
        {"movieId": 1, "title": "Storm", "type": "movie", "releaseYear": 1986, "imdbId": "tt0000002"},
        {"movieId": 2, "title": "Storm", "type": "movie", "releaseYear": 1995, "imdbId": "tt0000003"}
    ]});
    let by_id = Search {
        movie: true,
        imdb: Some("tt0000003".into()),
        ..Search::default()
    };
    assert_eq!(pick_title(&movies, &by_id), Some(2));
    let by_name = Search {
        movie: true,
        title: Some("storm".into()),
        year: Some(1995),
        ..Search::default()
    };
    assert_eq!(pick_title(&movies, &by_name), Some(2));
    let other_name = Search {
        title: Some("Storm Front".into()),
        ..by_name
    };
    assert_eq!(pick_title(&movies, &other_name), None);
    assert_eq!(pick_title(&Value::Null, &by_id), None);
}

#[test]
fn episode_rows_are_read_and_filtered() {
    let json: Value = serde_json::from_str(SUBTITLES).unwrap();
    let found = parse(&json, &episode(), "en");
    let ids: Vec<&str> = found.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["subsource:9001", "subsource:9003", "subsource:9005"]
    );
    assert_eq!(found[0].downloads, Some(1520));
    assert_eq!(
        found[0].release,
        "Some.Show.S01E02.720p.WEB.h264-GrP\nSome.Show.S01E02.1080p.WEB.h264-GrP"
    );
    assert!(!found[0].episode_unsure);
    assert_eq!(found[0].episode, Some((1, 2)));
    assert!(found[1].episode_unsure && found[1].hearing_impaired);
    assert_eq!(found[2].language, "pt-BR");
    assert!(!found[2].episode_unsure);

    let movie = Search {
        movie: true,
        imdb: Some("tt0000004".into()),
        ..Search::default()
    };
    let found = parse(&json, &movie, "en");
    assert_eq!(found.len(), 4);
    assert!(found
        .iter()
        .all(|item| !item.episode_unsure && item.episode.is_none()));
}

#[test]
fn languages_map_to_names_two_at_most() {
    assert_eq!(
        wanted(&langs(&["pt-BR", "eu", "en", "fr"])),
        vec![("pt-BR", "brazillian_portuguese"), ("en", "english")]
    );
    assert!(wanted(&langs(&["eu", "zh-CN"])).is_empty());
    assert_eq!(wanted(&langs(&["fa"])), vec![("fa", "farsi_persian")]);
    assert_eq!(language_of("English"), Some("en"));
    assert_eq!(language_of("farsi_persian"), Some("fa"));
    assert_eq!(language_of("klingon"), None);
}

#[test]
fn title_lookups_prefer_the_imdb_id() {
    let pairs = title_params(&episode()).unwrap();
    assert_eq!(
        pairs,
        vec![
            ("searchType", "imdb".to_string()),
            ("imdb", "tt0000001".to_string()),
            ("type", "series".to_string()),
            ("season", "1".to_string()),
        ]
    );
    let by_name = Search {
        movie: true,
        title: Some("Storm".into()),
        ..Search::default()
    };
    assert_eq!(
        title_params(&by_name).unwrap(),
        vec![
            ("searchType", "text".to_string()),
            ("q", "Storm".to_string())
        ]
    );
    assert_eq!(title_params(&Search::default()), None);
}

#[test]
fn refusals_explain_themselves() {
    assert_eq!(
        refusal(StatusCode::UNAUTHORIZED, KEY_REFUSED),
        SourceError::Note(KEY_REFUSED.into())
    );
    assert_eq!(
        refusal(StatusCode::FORBIDDEN, SAVED_KEY_REFUSED),
        SourceError::Note(SAVED_KEY_REFUSED.into())
    );
    assert_eq!(
        refusal(StatusCode::TOO_MANY_REQUESTS, KEY_REFUSED),
        SourceError::Note(BUSY.into())
    );
    assert_eq!(
        refusal(StatusCode::INTERNAL_SERVER_ERROR, KEY_REFUSED),
        SourceError::Failed
    );
    assert!(valid_id("9001"));
    assert!(!valid_id("90a1"));
    assert!(!valid_id(""));
}

#[test]
fn numbers_sent_as_strings_are_read() {
    let titles = serde_json::json!({"data": [
        {"movieId": "4100", "imdbId": "tt0000001", "season": "1"},
        {"movieId": "4105", "imdbId": "tt0000001", "season": "5"}
    ]});
    assert_eq!(pick_title(&titles, &episode()), Some(4100));
    // Season 3 is not listed: no other season stands in for it.
    let third = Search {
        episode: Some((3, 1)),
        ..episode()
    };
    assert_eq!(pick_title(&titles, &third), None);
    // A show listed without seasons is used for any season.
    let whole = serde_json::json!({"data": [{"movieId": 7, "imdbId": "tt0000001"}]});
    assert_eq!(pick_title(&whole, &third), Some(7));

    let rows = serde_json::json!({"data": [
            {"subtitleId": "9001", "language": "english", "releaseInfo": ["Some.Show.S01E02.WEB"], "downloads": "12"}
        ], "pagination": {"page": 1, "limit": 50, "total": "120"}});
    let found = parse(&rows, &episode(), "en");
    assert_eq!(found[0].id, "subsource:9001");
    assert_eq!(found[0].downloads, Some(12));
    assert!(more_pages(&rows, 1) && more_pages(&rows, 2) && !more_pages(&rows, 3));
    let json: Value = serde_json::from_str(SUBTITLES).unwrap();
    assert!(!more_pages(&json, 1));
}
