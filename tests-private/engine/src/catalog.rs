use super::*;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn folder(label: &str, path: &str, kind: ItemKind) -> BrowseItem {
    BrowseItem {
        id: path.to_string(),
        label: label.to_string(),
        path: path.to_string(),
        kind,
        art: None,
        title_kind: None,
        season: None,
        episode: None,
        show_title: None,
        show_path: None,
        settings_addon: None,
        info: None,
    }
}

#[test]
fn shows_and_movies_found_are_shown_together_with_the_named_title_first() {
    let shows = BrowsePage {
        path: "plugin://plugin.video.example/?action=tvSearchterm&name=Lantern".to_string(),
        items: vec![
            folder(
                "The Cruise",
                "plugin://plugin.video.example/?show=1",
                ItemKind::Open,
            ),
            folder(
                "Lantern Island",
                "plugin://plugin.video.example/?show=2",
                ItemKind::Open,
            ),
        ],
    };
    let movies = BrowsePage {
        path: "plugin://plugin.video.example/?action=movieSearchterm&name=Lantern".to_string(),
        items: vec![
            folder(
                "Lantern (2010)",
                "plugin://plugin.video.example/?movie=1",
                ItemKind::Play,
            ),
            folder(
                "The Cruise",
                "plugin://plugin.video.example/?show=1",
                ItemKind::Open,
            ),
        ],
    };
    let merged = merge_results(vec![shows, movies], " lantern ").unwrap();
    let labels: Vec<_> = merged
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, ["Lantern (2010)", "The Cruise", "Lantern Island"]);
    assert!(merged.path.contains("tvSearchterm"));
    assert!(merge_results(Vec::new(), "x").is_none());
}

#[test]
fn search_menu_lists_tv_shows_then_movies() {
    let page = BrowsePage {
        path: "plugin://plugin.video.example/?action=search".to_string(),
        items: vec![
            folder(
                "Movies",
                "plugin://plugin.video.example/?action=movies",
                ItemKind::Open,
            ),
            folder(
                "TV Shows",
                "plugin://plugin.video.example/?action=tv",
                ItemKind::Open,
            ),
            folder(
                "People - Movies",
                "plugin://plugin.video.example/?action=people-movies",
                ItemKind::Run,
            ),
            folder(
                "People - TV Shows",
                "plugin://plugin.video.example/?action=people",
                ItemKind::Run,
            ),
        ],
    };
    assert!(!search_section_paths(&page).is_empty());
    assert_eq!(
        search_section_paths(&page),
        [
            "plugin://plugin.video.example/?action=tv",
            "plugin://plugin.video.example/?action=movies"
        ]
    );
    let results = BrowsePage {
        path: "plugin://plugin.video.example/?action=tv".to_string(),
        items: vec![folder(
            "Example Cartoon",
            "plugin://plugin.video.example/show",
            ItemKind::Play,
        )],
    };
    assert!(search_section_paths(&results).is_empty());
    assert!(new_search_path(&results).is_none());
    let history = BrowsePage {
        path: "plugin://plugin.video.example/?action=tvsearch".to_string(),
        items: vec![
            folder(
                "New search...",
                "plugin://plugin.video.example/?action=new",
                ItemKind::Run,
            ),
            folder(
                "blacklist",
                "plugin://plugin.video.example/?action=blacklist",
                ItemKind::Open,
            ),
            folder(
                "Click to clear search history",
                "plugin://plugin.video.example/?action=clear",
                ItemKind::Run,
            ),
        ],
    };
    assert!(search_section_paths(&history).is_empty());
    assert_eq!(
        new_search_path(&history).as_deref(),
        Some("plugin://plugin.video.example/?action=new")
    );
    let saved = BrowsePage {
        path: "plugin://plugin.video.example/?action=tvsearch".to_string(),
        items: vec![
            folder(
                "New search...",
                "plugin://plugin.video.example/?action=new",
                ItemKind::Run,
            ),
            folder(
                "futurama",
                "plugin://plugin.video.example/?action=shows&name=futurama",
                ItemKind::Open,
            ),
            folder(
                "blacklist",
                "plugin://plugin.video.example/?action=blacklist",
                ItemKind::Open,
            ),
        ],
    };
    let (template, key) = search_template(&saved).expect("saved search");
    assert_eq!(key, "name");
    assert_eq!(
        with_query_value(&template, &key, "the show").as_deref(),
        Some("plugin://plugin.video.example/?action=shows&name=the%20show")
    );
}

#[test]
fn scraper_search_menu_prefers_shows_and_skips_folder_name() {
    let menu = BrowsePage {
        path: "plugin://plugin.video.example/?action=tools_searchNavigator".to_string(),
        items: vec![
            folder(
                "Search Movies...",
                "plugin://plugin.video.example/?action=movieSearch&folderName=Search+Movies...",
                ItemKind::Open,
            ),
            folder(
                "Search Shows...",
                "plugin://plugin.video.example/?action=tvSearch&folderName=Search+Shows...",
                ItemKind::Open,
            ),
        ],
    };
    assert!(!search_section_paths(&menu).is_empty());
    assert_eq!(
        search_section_paths(&menu),
        [
            "plugin://plugin.video.example/?action=tvSearch&folderName=Search+Shows...",
            "plugin://plugin.video.example/?action=movieSearch&folderName=Search+Movies..."
        ]
    );

    let empty_history = BrowsePage {
        path: "plugin://plugin.video.example/?action=tvSearch&folderName=Search+Shows..."
            .to_string(),
        items: vec![folder(
            "New search...",
            "plugin://plugin.video.example/?action=tvSearchnew&folderName=New+Search...",
            ItemKind::Run,
        )],
    };
    assert!(search_template(&empty_history).is_none());
    assert_eq!(
        new_search_path(&empty_history).as_deref(),
        Some("plugin://plugin.video.example/?action=tvSearchnew&folderName=New+Search...")
    );

    let with_history = BrowsePage {
        path: "plugin://plugin.video.example/?action=movieSearch&folderName=Search+Movies..."
            .to_string(),
        items: vec![
            folder(
                "New search...",
                "plugin://plugin.video.example/?action=movieSearchnew&folderName=New+Search...",
                ItemKind::Run,
            ),
            folder(
                "Search Movies...",
                "plugin://plugin.video.example/?action=movieSearch&folderName=Search+Movies...",
                ItemKind::Open,
            ),
            folder(
                "night train 2",
                "plugin://plugin.video.example/?action=movieSearchterm&name=night+train+2",
                ItemKind::Open,
            ),
        ],
    };
    // A menu row carrying its own label is never a saved search.
    let menu_only = BrowsePage {
        path: String::new(),
        items: vec![folder(
            "Search Movies...",
            "plugin://plugin.video.example/?action=movieSearch&folderName=Search+Movies...",
            ItemKind::Open,
        )],
    };
    assert!(search_template(&menu_only).is_none());
    let (template, key) = search_template(&with_history).expect("saved search");
    assert_eq!(key, "name");
    assert_eq!(
        with_query_value(&template, &key, "24").as_deref(),
        Some("plugin://plugin.video.example/?action=movieSearchterm&name=24")
    );
    // No URL convention is assumed: the add-on's own New search row runs.
    match listing_search_plan(&empty_history, "24") {
        ListingSearch::Run(url) => assert!(url.contains("action=tvSearchnew")),
        other => panic!("a New search Run row is started, got {other:?}"),
    }
}

#[test]
fn a_run_search_row_is_started_instead_of_opened_as_a_folder() {
    let history = BrowsePage {
            path: "plugin://plugin.video.example/?mode=navigator.search_history&action=movie"
                .to_string(),
            items: vec![
                folder(
                    "NEW SEARCH...",
                    "plugin://plugin.video.example/?mode=search.get_key_id&media_type=movie&isFolder=false",
                    ItemKind::Run,
                ),
                folder(
                    "Lantern",
                    "plugin://plugin.video.example/?mode=search.get_key_id&media_type=movie&key_id=Lantern&isFolder=false",
                    ItemKind::Run,
                ),
            ],
        };
    match listing_search_plan(&history, "Heat") {
        ListingSearch::Run(url) => {
            assert!(url.contains("key_id=Heat"));
            assert!(!url.contains("key_id=Lantern"));
            assert!(url.contains("isFolder=false"));
        }
        other => panic!("a saved Run search is run, got {other:?}"),
    }
    let fresh = BrowsePage {
            path: "plugin://plugin.video.example/?mode=navigator.search_history".to_string(),
            items: vec![folder(
                "NEW SEARCH...",
                "plugin://plugin.video.example/?mode=search.get_key_id&media_type=tv_show&isFolder=false",
                ItemKind::Run,
            )],
        };
    match listing_search_plan(&fresh, "Heat") {
        ListingSearch::Run(url) => {
            assert!(url.contains("mode=search.get_key_id"));
            assert!(!url.contains("key_id="));
        }
        other => panic!("an empty Run search is run, got {other:?}"),
    }
    let saved_folder = BrowsePage {
        path: "plugin://plugin.video.example/?action=tvsearch".to_string(),
        items: vec![folder(
            "futurama",
            "plugin://plugin.video.example/?action=shows&name=futurama",
            ItemKind::Open,
        )],
    };
    match listing_search_plan(&saved_folder, "the show") {
        ListingSearch::Browse(url) => {
            assert_eq!(
                url,
                "plugin://plugin.video.example/?action=shows&name=the%20show"
            );
        }
        other => panic!("a folder history row is browsed, got {other:?}"),
    }
}

#[test]
fn a_search_folder_counts_only_when_it_is_a_different_plugin() {
    let before = "plugin://plugin.video.example/?mode=navigator.search_history";
    let ran = "plugin://plugin.video.example/?mode=search.get_key_id&isFolder=false";
    let results =
        "plugin://plugin.video.example/?mode=build_movie_list&action=tmdb_movies_search&query=Heat";
    assert_eq!(
        changed_search_folder(before, ran, results).as_deref(),
        Some(results)
    );
    assert!(changed_search_folder(before, ran, ran).is_none());
    assert!(changed_search_folder(before, ran, before).is_none());
    assert!(changed_search_folder(before, ran, &format!("{before}/")).is_none());
    assert!(changed_search_folder(before, ran, "special://home").is_none());
    assert!(changed_search_folder("", ran, ran).is_none());
}

#[test]
fn the_root_search_row_may_end_in_dots() {
    let root = BrowsePage {
        path: "plugin://plugin.video.example/".to_string(),
        items: vec![
            folder(
                "Search history",
                "plugin://plugin.video.example/?action=history",
                ItemKind::Open,
            ),
            folder(
                "Search...",
                "plugin://plugin.video.example/?action=search",
                ItemKind::Open,
            ),
        ],
    };
    assert_eq!(
        search_root_path(&root).as_deref(),
        Some("plugin://plugin.video.example/?action=search")
    );
    let none = BrowsePage {
        path: "plugin://plugin.video.example/".to_string(),
        items: vec![folder(
            "Movies",
            "plugin://plugin.video.example/?action=movies",
            ItemKind::Open,
        )],
    };
    assert!(search_root_path(&none).is_none());
}

#[test]
fn a_saved_search_with_accents_or_a_bare_flag_is_reused() {
    assert_eq!(decode_query("caf%C3%A9+cr%C3%A8me"), "caf\u{e9} cr\u{e8}me");
    assert_eq!(decode_query("caf\u{e9}"), "caf\u{e9}");
    assert_eq!(decode_query("100%"), "100%");
    let saved = BrowsePage {
        path: "plugin://plugin.video.example/?action=tvsearch".to_string(),
        items: vec![
            folder(
                "New search...",
                "plugin://plugin.video.example/?action=new",
                ItemKind::Run,
            ),
            folder(
                "caf\u{e9}",
                "plugin://plugin.video.example/?action=shows&refresh&name=caf%C3%A9",
                ItemKind::Open,
            ),
        ],
    };
    let (template, key) = search_template(&saved).expect("saved search");
    assert_eq!(key, "name");
    assert_eq!(
        with_query_value(&template, &key, "\u{e9}t\u{e9}").as_deref(),
        Some("plugin://plugin.video.example/?action=shows&refresh&name=%C3%A9t%C3%A9")
    );
}

#[test]
fn play_refusal_blocks_settings_and_search_actions() {
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=openSettings",
            "Settings"
        ),
        Some("That item opens add-on settings.")
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=tvSearchnew&folderName=New+Search...",
            "New search..."
        ),
        Some("That item is an add-on action, not a video.")
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=clearSearchHistory",
            "Clear search history"
        ),
        Some("That item is an add-on action, not a video.")
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=people",
            "People - Movies"
        ),
        Some("That item is an add-on action, not a video.")
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=play&title=Show",
            "Show"
        ),
        None
    );
    assert!(plugin_link(
        "RunPlugin(plugin://plugin.video.example/?action=tools)"
    ));
    assert!(!plugin_link("https://cdn.example/a.mp4"));
    assert_eq!(
        run_plugin_builtin("plugin://plugin.video.example/?q=a,b&j=\"x\""),
        "RunPlugin(\"plugin://plugin.video.example/?q=a,b&j=\\\"x\\\"\")"
    );
}

#[test]
fn play_refusal_blocks_utility_tools_auth_and_cache() {
    let refused = Some("That item is an add-on action, not a video.");
    assert_eq!(
        play_refusal("plugin://plugin.video.example/?action=tools", "Tools"),
        refused
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=tools_listTools",
            "Lists Manager"
        ),
        refused
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=acct_Authorize",
            "Authorize Example Host"
        ),
        refused
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=clearCache",
            "Clear cache"
        ),
        refused
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=cleanSettings",
            "Clean Settings"
        ),
        refused
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=tools_viewLogFile",
            "Log Viewer"
        ),
        refused
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=helpNavigator",
            "Help"
        ),
        refused
    );
    assert_eq!(
        play_refusal(
            "plugin://plugin.video.example/?action=playItem&title=Lantern",
            "Lantern"
        ),
        None
    );
}

#[test]
fn play_refusal_leaves_a_video_file_alone() {
    assert_eq!(play_refusal(r"D:\Help.mkv", "Help"), None);
    assert_eq!(play_refusal("https://cdn.example/Tools.mp4", "Tools"), None);
}

#[test]
fn a_root_without_a_search_row_still_has_its_search_entries() {
    let root = BrowsePage {
        path: "plugin://plugin.video.example/".into(),
        items: vec![
            folder(
                "Movies",
                "plugin://plugin.video.example/?action=movies",
                ItemKind::Open,
            ),
            folder(
                "Search history",
                "plugin://plugin.video.example/?action=history",
                ItemKind::Open,
            ),
            folder(
                "Search Movies...",
                "plugin://plugin.video.example/?action=movieSearch",
                ItemKind::Open,
            ),
            folder(
                "Search Shows...",
                "plugin://plugin.video.example/?action=tvSearch",
                ItemKind::Run,
            ),
        ],
    };
    assert!(search_root_path(&root).is_none());
    assert_eq!(
        root_search_entries(&root),
        [
            "plugin://plugin.video.example/?action=tvSearch",
            "plugin://plugin.video.example/?action=movieSearch",
        ]
    );
}

#[test]
fn settings_rows_point_at_the_form() {
    let marked = browse_item(&json!({
        "filetype": "file",
        "label": "[COLOR orchid]\u{00A4}SOME MODULE : Settings\u{00A4}[/COLOR]",
        "file": "plugin://plugin.video.example/?action=moduleTools"
    }));
    assert_eq!(marked.label, "SOME MODULE : Settings");
    // Another add-on's form is looked up by name among installed add-ons.
    assert!(marked.settings_addon.is_none());
    assert_eq!(marked.kind, ItemKind::Run);
    assert_eq!(
        settings_name_hint(&marked.path, &marked.label).as_deref(),
        Some("SOME MODULE")
    );
    assert_eq!(
        settings_name_hint(
            "RunPlugin(plugin://plugin.video.example/?action=SomeModuleSettings&query=1)",
            ""
        )
        .as_deref(),
        Some("SomeModule")
    );
    assert_eq!(
        settings_name_hint("plugin://plugin.video.example/?action=movies", "Settings"),
        None
    );
    assert_eq!(
        settings_name_hint("plugin://plugin.video.example/?action=tools", "Tools"),
        None
    );
    assert_eq!(
        settings_addon("plugin://plugin.video.example/?action=openSettings", "").as_deref(),
        Some("plugin.video.example")
    );
    assert_eq!(
        settings_addon(
            "plugin://plugin.video.example/?action=openSettings&query=11.0",
            ""
        )
        .as_deref(),
        Some("plugin.video.example")
    );
    assert_eq!(
            settings_addon(
                "ActivateWindow(10025,\"plugin://plugin.video.example/?action=openSettings&query=11.0\",return)",
                "Settings"
            )
            .as_deref(),
            Some("plugin.video.example")
        );
    assert_eq!(
        settings_addon("plugin://plugin.video.example/?action=movies", "Settings"),
        None
    );
    assert_eq!(
        settings_addon("plugin://plugin.video.example/?action=authAccount", ""),
        None
    );

    let settings_folder = browse_item(&json!({
        "filetype": "directory",
        "label": "Settings",
        "file": "plugin://plugin.video.example/?action=movies"
    }));
    assert_eq!(settings_folder.kind, ItemKind::Open);
    assert!(settings_folder.settings_addon.is_none());
}

#[test]
fn plain_label_strips_kodi_markup() {
    assert_eq!(plain_label("\u{00A4} Movies \u{00A4}"), "Movies");
    assert_eq!(plain_label("[COLOR orchid]Settings[/COLOR]"), "Settings");
    assert_eq!(plain_label(""), "");
    assert_eq!(plain_label("[B][I]Settings[/I][/B]"), "Settings");
    assert_eq!(plain_label("[UPPERCASE]Hi[/UPPERCASE]"), "Hi");
    assert_eq!(plain_label("A[CR]B"), "A B");
    assert_eq!(plain_label("A[CR][CR]B"), "A B");
    assert_eq!(
        plain_label("[COLOR orchid][B]Settings[/B][/COLOR]"),
        "Settings"
    );
    assert_eq!(plain_label("A[TABS]B"), "A B");
}

#[test]
fn plain_block_strips_scrape_markup_and_keeps_lines() {
    let raw = "[B][COLOR white]\u{00A4} [/COLOR][/B][COLOR orchid]Paid[/COLOR]: 4K: [COLOR red][B]0[/B][/COLOR] | 1080p: [COLOR lime][B]10[/B][/COLOR]\n[COLOR orchid]Free[/COLOR]: 4K: [COLOR red][B]0[/B][/COLOR]\n[COLOR orchid]Remaining providers[/COLOR]: SCENERLS | 32/33 complete.";
    assert_eq!(
        plain_block(raw),
        "Paid: 4K: 0 | 1080p: 10\nFree: 4K: 0\nRemaining providers: SCENERLS | 32/33 complete."
    );
    assert_eq!(plain_block(""), "");
}

#[test]
fn classify_directory_opens() {
    let item = json!({
        "filetype": "directory",
        "file": "plugin://plugin.video.example/?action=movies"
    });
    assert_eq!(classify(&item), ItemKind::Open);
}

#[test]
fn classify_non_plugin_file_plays() {
    let item = json!({
        "filetype": "file",
        "file": "smb://nas/Movies/A.mkv"
    });
    assert_eq!(classify(&item), ItemKind::Play);
}

#[test]
fn classify_isplayable_true_plays() {
    let item = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=openSettings",
        "isplayable": true,
        "label": "Settings"
    });
    assert_eq!(classify(&item), ItemKind::Play);
    let text = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=openSettings",
        "isplayable": "true"
    });
    assert_eq!(classify(&text), ItemKind::Play);
}

#[test]
fn classify_isplayable_false_runs_even_for_mkv() {
    let item = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=openSettings",
        "isplayable": false,
        "type": "movie",
        "label": "Show.mkv"
    });
    assert_eq!(classify(&item), ItemKind::Run);
    let text = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=openSettings",
        "isplayable": "false",
        "label": "Clip.mkv"
    });
    assert_eq!(classify(&text), ItemKind::Run);
}

#[test]
fn classify_movie_type_plays() {
    let item = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=play",
        "type": "movie"
    });
    assert_eq!(classify(&item), ItemKind::Play);
}

#[test]
fn classify_label_mp4_plays_and_other_rows_run() {
    let video = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=cloud",
        "label": "Show.mp4"
    });
    assert_eq!(classify(&video), ItemKind::Play);
    let other = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=tools",
        "label": "Settings"
    });
    assert_eq!(classify(&other), ItemKind::Run);
    let mpeg = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=tools",
        "label": "clip.mpeg"
    });
    assert_eq!(classify(&mpeg), ItemKind::Play);
    let mpgx = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=tools",
        "label": "clip.mpgx"
    });
    assert_eq!(classify(&mpgx), ItemKind::Run);
}

#[test]
fn classify_decoded_extension_plays_and_bad_escapes_stay_raw() {
    let decoded = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.x/?u=Show%2Emkv"
    });
    assert_eq!(classify(&decoded), ItemKind::Play);
    let spaced = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.x/?u=file.mkv%20cut"
    });
    assert_eq!(classify(&spaced), ItemKind::Play);
    let bad = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.x/?u=name%2Emkv %GG"
    });
    assert_eq!(classify(&bad), ItemKind::Run);
}

#[test]
fn classify_non_object_runs() {
    assert_eq!(classify(&Value::Null), ItemKind::Run);
    assert_eq!(classify(&json!("plugin://video.example/")), ItemKind::Run);
    assert_eq!(classify(&json!([1, 2])), ItemKind::Run);
}

#[test]
fn directory_params_plugin_uses_files_without_isplayable() {
    let params = directory_params("plugin://plugin.video.example/");
    assert_eq!(params["media"], "files");
    assert_eq!(params["directory"], "plugin://plugin.video.example/");
    assert_eq!(params["properties"], json!(directory_properties()));
    assert!(params.get("isplayable").is_none());
    assert!(!params.to_string().contains("isplayable"));
    assert_eq!(
        directory_params("  Plugin://addon.id/path")["media"],
        "files"
    );
}

#[test]
fn directory_properties_are_the_documented_file_fields() {
    // Every name must be in JSON-RPC List.Fields.Files, or Kodi rejects the call.
    assert_eq!(
        directory_properties(),
        vec![
            "title",
            "thumbnail",
            "art",
            "plot",
            "plotoutline",
            "tagline",
            "year",
            "premiered",
            "rating",
            "votes",
            "genre",
            "mpaa",
            "runtime",
            "studio",
            "director",
            "writer",
            "cast",
            "imdbnumber",
            "uniqueid",
            "originaltitle",
            "trailer",
            "showtitle",
            "season",
            "episode",
            "country",
            "duration",
        ]
    );
    let mut seen = std::collections::BTreeSet::new();
    assert!(directory_properties().iter().all(|name| seen.insert(*name)));
    assert!(!directory_properties().contains(&"isplayable"));
}

#[test]
fn browse_items_carry_add_on_info() {
    let result = json!({
        "files": [
            {
                "filetype": "file",
                "file": "plugin://plugin.video.example/?action=play&id=1",
                "label": "Example",
                "type": "movie",
                "title": "Example",
                "year": 1994,
                "plot": "A plot.",
                "imdbnumber": "tt0111161",
                "uniqueid": {},
                "genre": [],
                "cast": [],
                "season": -1,
                "episode": -1,
                "art": {"poster": "image://poster.jpg/"}
            },
            {
                "filetype": "directory",
                "file": "plugin://plugin.video.example/?action=menu",
                "label": "Menu",
                "title": "",
                "year": 0,
                "plot": "",
                "uniqueid": {},
                "genre": [],
                "cast": []
            }
        ]
    });
    let page = page_from("plugin://plugin.video.example/", &result);
    let info = page.items[0].info.as_ref().unwrap();
    assert_eq!(info.year, Some(1994));
    assert_eq!(info.imdb_id.as_deref(), Some("tt0111161"));
    assert_eq!(info.poster.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(page.items[0].art.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(page.items[1].info, None);
    let json = serde_json::to_value(&page.items[1]).unwrap();
    assert!(json.get("info").is_none());
}

#[test]
fn execute_params_keep_query_and_path() {
    assert_eq!(
        plugin_execute_params("RunPlugin(plugin://plugin.video.example/?action=acct_Authorize)"),
        "/?action=acct_Authorize"
    );
    assert_eq!(
        plugin_execute_params("plugin://plugin.video.demo/?action=ad_Authorize"),
        "/?action=ad_Authorize"
    );
    assert_eq!(
        plugin_execute_params("plugin://plugin.video.demo/tools/clean"),
        "/tools/clean"
    );
    assert_eq!(plugin_execute_params("plugin://plugin.video.demo/"), "");
}

#[test]
fn directory_params_normal_path_uses_video() {
    let params = directory_params("smb://nas/Movies");
    assert_eq!(params["media"], "video");
    assert_eq!(params["directory"], "smb://nas/Movies");
    assert_eq!(params["properties"], json!(directory_properties()));
    assert!(!params.to_string().contains("isplayable"));
}

#[test]
fn episode_codes_parse_from_labels() {
    assert_eq!(parse_season_episode("Show S01E02 Pilot"), Some((1, 2)));
    assert_eq!(parse_season_episode("s1e2"), Some((1, 2)));
    assert_eq!(parse_season_episode("1x03"), Some((1, 3)));
    assert_eq!(parse_season_episode("Pilot"), None);
    assert_eq!(parse_season_episode("Show S00E01"), Some((0, 1)));
    assert_eq!(
        season_episode(&json!({"season": 0, "episode": 1}), "Special"),
        (Some(0), Some(1))
    );
    assert_eq!(
        season_episode(&json!({"label": "Show S00E01"}), "Show S00E01"),
        (Some(0), Some(1))
    );
    assert_eq!(
        season_episode(&json!({"season": 0}), "Folder"),
        (None, None)
    );
    assert_eq!(
        season_episode(&json!({"season": -1, "episode": 1}), "x"),
        (None, Some(1))
    );
}

#[tokio::test]
async fn a_search_that_never_opens_a_keyboard_times_out() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        while matches!(socket.read(&mut buf).await, Ok(count) if count > 0) {}
    });
    let browse_client = KodiClient::new("127.0.0.1", port);
    browse_client.connect().await.unwrap();
    let dialog_client = KodiClient::new("127.0.0.1", port);
    tokio::time::pause();
    let result = open_search(
        &browse_client,
        &dialog_client,
        "plugin://plugin.video.example/?action=search",
        "heat",
    )
    .await;
    match result {
        Err(err) => assert_eq!(err.to_string(), "The add-on took too long to search."),
        Ok(_) => panic!("a search with no keyboard must not count as typed"),
    }
    browse_client.close().await;
}

#[tokio::test]
async fn browse_keeps_kodi_order_and_plain_labels() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 2048];
        let request = loop {
            let count = socket.read(&mut buf).await.unwrap();
            assert!(count > 0);
            raw.extend_from_slice(&buf[..count]);
            if let Ok(value) = serde_json::from_slice::<Value>(&raw) {
                break value;
            }
        };
        assert_eq!(request["method"], "Files.GetDirectory");
        assert_eq!(request["params"]["media"], "files");
        assert_eq!(
            request["params"]["properties"],
            json!(directory_properties())
        );
        assert!(request["params"].get("isplayable").is_none());
        let id = request["id"].as_i64().unwrap();
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "files": [
                    {
                        "filetype": "file",
                        "file": "plugin://video.example/?z=1",
                        "label": "[B]Zed[/B]"
                    },
                    {
                        "filetype": "directory",
                        "file": "plugin://video.example/folder",
                        "label": "[COLOR orchid]Alpha[/COLOR]"
                    }
                ]
            }
        });
        socket
            .write_all(&serde_json::to_vec(&body).unwrap())
            .await
            .unwrap();
        socket.flush().await.unwrap();
        let _ = socket.read(&mut buf).await;
    });

    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let page = browse(&client, "plugin://video.example/").await.unwrap();
    assert_eq!(page.path, "plugin://video.example/");
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].label, "Zed");
    assert_eq!(page.items[1].label, "Alpha");
    assert_eq!(page.items[0].path, "plugin://video.example/?z=1");
    assert_eq!(page.items[1].path, "plugin://video.example/folder");
    assert_eq!(page.items[0].id, "plugin://video.example/?z=1");
    client.close().await;
}
