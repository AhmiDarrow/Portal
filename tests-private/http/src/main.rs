use super::*;

#[test]
fn a_new_showing_of_the_same_list_is_not_picked_again_at_once() {
    let base = "skin:source_results.xml";
    assert_eq!(prompt_showing_base("skin:source_results.xml-6"), base);
    assert_eq!(prompt_showing_base("w12000-3"), "w12000");
    assert_eq!(prompt_showing_base("plain"), "plain");
    assert!(hold_source_pick(
        base,
        "skin:source_results.xml-7",
        false,
        Duration::from_secs(1)
    ));
    assert!(!hold_source_pick(
        base,
        "skin:source_results.xml-7",
        false,
        SOURCE_PICK_RETRY
    ));
    // The list went away and came back: that copy did not play.
    assert!(!hold_source_pick(
        base,
        "skin:source_results.xml-8",
        true,
        Duration::from_secs(1)
    ));
    assert!(!hold_source_pick(
        "",
        "skin:source_results.xml-1",
        true,
        Duration::ZERO
    ));
    assert!(!hold_source_pick(
        base,
        "skin:other.xml-1",
        false,
        Duration::from_secs(1)
    ));
    assert!(!source_pick_waits(
        "Kodi's list is not ready yet. Try again, or cancel."
    ));
    assert!(source_pick_waits("That prompt is gone."));
}

#[test]
fn a_picked_row_counts_only_after_its_list_goes_away() {
    let mut attempts = SourceAttempts::new();
    attempts.note_list_left();
    assert!(attempts.tried.is_empty());
    attempts.base = "skin:source_results.xml".into();
    attempts.pending = Some("Movie.1080p".into());
    attempts.list_left = false;
    attempts.note_list_left();
    assert_eq!(attempts.tried, vec!["Movie.1080p".to_string()]);
    assert!(attempts.pending.is_none());
    attempts.note_list_left();
    assert_eq!(attempts.tried.len(), 1);
}

#[test]
fn the_same_list_is_left_to_the_viewer_after_answers_keep_failing() {
    let mut attempts = SourceAttempts::new();
    let start = Instant::now();
    let base = "skin:source_results.xml";
    attempts.note_failure(base, start);
    assert!(!attempts.failures_expired(start));
    assert!(!attempts.failures_expired(start + SOURCE_PICK_RETRY - Duration::from_millis(1)));
    assert!(attempts.failures_expired(start + SOURCE_PICK_RETRY));
    // The same list keeps the original clock. A flicker away does not reset it.
    attempts.note_failure(base, start + Duration::from_secs(3));
    assert!(attempts.failures_expired(start + SOURCE_PICK_RETRY));
    attempts.note_list_left();
    assert!(attempts.failures_expired(start + SOURCE_PICK_RETRY));
    // A different list starts again.
    attempts.note_failure("skin:other.xml", start + SOURCE_PICK_RETRY);
    assert!(!attempts.failures_expired(start + SOURCE_PICK_RETRY));
    assert!(attempts.failures_expired(start + SOURCE_PICK_RETRY + SOURCE_PICK_RETRY));
    attempts.clear_failure();
    assert!(!attempts.failures_expired(start + SOURCE_PICK_RETRY * 3));
}

#[test]
fn plugin_urls_come_out_of_addon_actions() {
    assert_eq!(
        plugin_target("RunPlugin(plugin://plugin.video.example/?mode=1)"),
        Some("plugin://plugin.video.example/?mode=1".to_string())
    );
    assert_eq!(
        plugin_target("ActivateWindow(10025,\"plugin://plugin.video.example/\",return)"),
        Some("plugin://plugin.video.example/".to_string())
    );
    assert_eq!(plugin_target("Nothing"), None);
}

#[test]
fn a_foreign_restart_is_a_conflict() {
    let (code, body) = from_engine(EngineError::ForeignRestart);
    assert_eq!(code, StatusCode::CONFLICT);
    let text = body.0.detail.as_str();
    assert!(text.contains("already running"));
    assert!(!text.to_ascii_lowercase().contains("password"));
}

#[test]
fn not_found_uses_a_public_sentence() {
    let (code, body) = from_engine(EngineError::NotFound);
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert_eq!(body.0.detail, "That item is gone.");
}

#[test]
fn bad_request_keeps_the_engine_sentence() {
    let (code, body) = from_engine(EngineError::BadRequest(
        "That item is an add-on action, not a video.".into(),
    ));
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(
        body.0.detail,
        "That item is an add-on action, not a video."
    );
    let (code, body) = from_engine(EngineError::BadRequest("Enter a name.".into()));
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(body.0.detail, "Enter a name.");
}

#[test]
fn list_put_name_rejects_blank_and_keeps_omitted() {
    assert_eq!(list_put_name(None).unwrap(), "");
    assert_eq!(list_put_name(Some("  Show  ")).unwrap(), "Show");
    let err = list_put_name(Some("   ")).unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    assert_eq!(err.1 .0.detail, "Enter a name.");
    let err = list_put_name(Some("")).unwrap_err();
    assert_eq!(err.1 .0.detail, "Enter a name.");
}

#[test]
fn saved_answers_for_every_id_played_or_not() {
    let path = std::env::temp_dir().join(format!("portal-http-saved-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut store = Store::open(&path).unwrap();
    let movie = "plugin://plugin.video.example/?action=play&title=Example&year=1994";
    let show = "plugin://plugin.video.example/?action=seasons&id=7";
    store
        .put_watchlist_with(
            movie,
            TitleKind::Movie,
            "Example",
            None,
            TitleFacts::new(Some(1994), Some("tt0111161")),
        )
        .unwrap();
    store
        .put_bookmark(show, TitleKind::Tv, "Show", None)
        .unwrap();
    let ids = vec![
        movie.to_string(),
        show.to_string(),
        "plugin://plugin.video.example/?other".to_string(),
        "  ".to_string(),
    ];
    let map = saved_map(&store, &ids);
    let json = serde_json::to_value(&map).unwrap();
    assert_eq!(
        json,
        json!({"items": {
            movie: {"watchlist": true, "bookmarks": false},
            show: {"watchlist": false, "bookmarks": true},
            "plugin://plugin.video.example/?other": {"watchlist": false, "bookmarks": false}
        }})
    );
    assert_eq!(
        store.title(movie).unwrap().imdb_id.as_deref(),
        Some("tt0111161")
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn play_refusal_matches_http_play_guard() {
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=rd_Authorize",
            "Authorize"
        ),
        Some("That item is an add-on action, not a video.")
    );
    assert_eq!(
        play_refusal("plugin://plugin.video.example/?action=tools", "Tools"),
        Some("That item is an add-on action, not a video.")
    );
}

#[test]
fn media_names_pick_a_content_type() {
    assert!(media_type("index.m3u8").contains("mpegurl"));
    assert!(media_type("sub00001.vtt").contains("vtt"));
    assert_eq!(media_type("seg00001.ts"), "video/mp2t");
    assert_eq!(media_type("cast.mp4"), "video/mp4");
    assert_eq!(media_type("movie.m4v"), "video/mp4");
}

#[test]
fn byte_ranges_seek_instead_of_reading_the_file() {
    assert_eq!(
        media_span(None, 1_500_000_000),
        MediaSpan::Full { len: 1_500_000_000 }
    );
    assert_eq!(
        media_span(Some("bytes=0-1048575"), 1_500_000_000),
        MediaSpan::Partial {
            start: 0,
            end: 1_048_575,
            total: 1_500_000_000
        }
    );
    assert_eq!(
        media_span(Some("bytes=4-"), 10),
        MediaSpan::Partial {
            start: 4,
            end: 9,
            total: 10
        }
    );
    assert_eq!(
        media_span(Some("bytes=-2"), 10),
        MediaSpan::Partial {
            start: 8,
            end: 9,
            total: 10
        }
    );
    assert_eq!(
        media_span(Some("bytes=0-1,2-3"), 10),
        MediaSpan::Partial {
            start: 0,
            end: 1,
            total: 10
        }
    );
    assert_eq!(
        media_span(Some("bytes=10-"), 10),
        MediaSpan::Unsatisfiable { total: 10 }
    );
    assert_eq!(
        media_span(Some("items=0-1"), 10),
        MediaSpan::Full { len: 10 }
    );
    assert_eq!(
        media_span(Some("bytes=-0"), 10),
        MediaSpan::Unsatisfiable { total: 10 }
    );
}

#[test]
fn media_responses_allow_any_origin_and_partial_content() {
    let partial = media_response(
        "video/mp4",
        MediaSpan::Partial {
            start: 0,
            end: 3,
            total: 10,
        },
        Body::empty(),
    );
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    let headers = partial.headers();
    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "*"
    );
    assert_eq!(headers.get(header::CONTENT_RANGE).unwrap(), "bytes 0-3/10");
    assert_eq!(headers.get(header::CONTENT_LENGTH).unwrap(), "4");
    assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "video/mp4");
    assert_eq!(headers.get(header::ACCEPT_RANGES).unwrap(), "bytes");

    let full = media_response("video/mp2t", MediaSpan::Full { len: 10 }, Body::empty());
    assert_eq!(full.status(), StatusCode::OK);
    assert!(full.headers().get(header::CONTENT_RANGE).is_none());
    assert_eq!(full.headers().get(header::CONTENT_LENGTH).unwrap(), "10");

    let bad = media_response(
        "video/mp4",
        MediaSpan::Unsatisfiable { total: 10 },
        Body::empty(),
    );
    assert_eq!(bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        bad.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes */10"
    );
}

#[test]
fn a_settings_window_kodi_opened_names_its_add_on() {
    let dir = std::env::temp_dir().join(format!(
        "portal-opened-settings-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for (id, name) in [("plugin.video.one", "One - Two"), ("script.other", "Other")] {
        let addon = dir.join("addons").join(id);
        std::fs::create_dir_all(addon.join("resources")).unwrap();
        std::fs::write(
            addon.join("addon.xml"),
            format!(r#"<addon id="{id}" name="{name}" version="1.0.0"></addon>"#),
        )
        .unwrap();
        std::fs::write(addon.join("resources").join("settings.xml"), "<settings/>").unwrap();
    }
    let settings = Settings::new(dir.clone(), dir.join("none"));
    // Kodi's word for "Settings" changes with its language; the name does not.
    assert_eq!(
        opened_settings_addon(&settings, "Settings - Other").as_deref(),
        Some("script.other")
    );
    assert_eq!(
        opened_settings_addon(&settings, "Einstellungen - One - Two").as_deref(),
        Some("plugin.video.one")
    );
    // A heading that names nothing belongs to the add-on just run.
    *LAST_RUN.lock().unwrap() = None;
    assert_eq!(opened_settings_addon(&settings, "Settings - Missing"), None);
    note_run("script.other");
    assert_eq!(
        opened_settings_addon(&settings, "").as_deref(),
        Some("script.other")
    );
    note_run("not.installed");
    assert_eq!(opened_settings_addon(&settings, ""), None);
    *LAST_RUN.lock().unwrap() = None;
    assert_eq!(
        plugin_addon("RunPlugin(plugin://plugin.video.one/?action=x)").as_deref(),
        Some("plugin.video.one")
    );
    assert_eq!(plugin_addon("plugin://"), None);
    assert_eq!(plugin_addon("special://home"), None);
    let _ = std::fs::remove_dir_all(&dir);
}
