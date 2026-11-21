use super::*;

fn temp_store(name: &str) -> Store {
    let path =
        std::env::temp_dir().join(format!("portal-library-{name}-{}.json", std::process::id()));
    let _ = fs::remove_file(&path);
    Store::open(&path).unwrap()
}

fn episode(show: &str, season: u32, episode: u32) -> BrowseItem {
    BrowseItem {
        id: format!("{show}/{season}/{episode}"),
        label: format!("S{season}E{episode} Name"),
        path: format!("plugin://example/play/{season}/{episode}"),
        kind: ItemKind::Play,
        art: None,
        title_kind: Some(TitleKind::Tv),
        season: Some(season),
        episode: Some(episode),
        show_title: Some("Example".into()),
        show_path: Some(show.into()),
        settings_addon: None,
        info: None,
    }
}

#[test]
fn bookmark_and_watchlist_survive_reload_and_stay_separate() {
    let path =
        std::env::temp_dir().join(format!("portal-library-lists-{}.json", std::process::id()));
    let _ = fs::remove_file(&path);
    let show = "plugin://plugin.video.example/?show=1";
    {
        let mut store = Store::open(&path).unwrap();
        store
            .put_bookmark(show, TitleKind::Tv, "Example", None)
            .unwrap();
        store
            .put_watchlist(show, TitleKind::Tv, "Example", None)
            .unwrap();
        store.delete_watchlist(show).unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.bookmarks().len(), 1);
    assert!(store.watchlist().is_empty());
    assert!(store.bookmarks()[0].id == show);
    let _ = fs::remove_file(&path);
}

#[test]
fn an_unplayed_plugin_title_saves_with_its_year_and_imdb_id() {
    let path = std::env::temp_dir().join(format!(
        "portal-library-unplayed-{}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    let movie =
        "plugin://plugin.video.example/?action=play&title=Example%20Movie&year=1994&meta={\"a\":1}";
    let show = "plugin://plugin.video.example/?action=seasons&tvshowtitle=Example&id=5:1:2";
    {
        let mut store = Store::open(&path).unwrap();
        assert!(store.title(movie).is_none());
        store
            .put_watchlist_with(
                movie,
                TitleKind::Movie,
                "Example Movie",
                Some("image://poster.jpg/".into()),
                TitleFacts::new(Some(1994), Some("https://www.imdb.com/title/tt0111161/")),
            )
            .unwrap();
        store
            .put_bookmark_with(
                show,
                TitleKind::Tv,
                "Example Show",
                None,
                TitleFacts::new(Some(99), Some("81189")),
            )
            .unwrap();
        // A later save without facts keeps them.
        store
            .put_bookmark(movie, TitleKind::Movie, "Example Movie", None)
            .unwrap();
    }
    let mut store = Store::open(&path).unwrap();
    let card = &store.watchlist()[0];
    assert_eq!(card.id, movie);
    assert_eq!(card.year, Some(1994));
    assert_eq!(card.imdb_id.as_deref(), Some("tt0111161"));
    assert_eq!(card.file.as_deref(), Some(movie));
    let detail = store.title(movie).unwrap();
    assert!(detail.watchlisted && detail.bookmarked);
    assert_eq!(detail.year, Some(1994));
    assert_eq!(detail.imdb_id.as_deref(), Some("tt0111161"));
    assert_eq!(detail.art.as_deref(), Some("image://poster.jpg/"));

    let show_detail = store.title(show).unwrap();
    assert!(show_detail.bookmarked && !show_detail.watchlisted);
    assert_eq!(show_detail.year, None);
    assert_eq!(show_detail.imdb_id, None);

    assert_eq!(
        store.saved_state(movie),
        SavedState {
            watchlist: true,
            bookmarks: true
        }
    );
    assert_eq!(
        store.saved_state(show),
        SavedState {
            watchlist: false,
            bookmarks: true
        }
    );
    assert_eq!(store.saved_state("plugin://other/"), SavedState::default());

    // An id that ends like an episode key still deletes.
    store.delete_bookmark(show).unwrap();
    assert_eq!(store.saved_state(show), SavedState::default());
    let _ = fs::remove_file(&path);
}

#[test]
fn a_library_file_from_before_facts_still_loads() {
    let path = std::env::temp_dir().join(format!("portal-library-old-{}.json", std::process::id()));
    let old = r#"{
            "titles": {"movie:plugin://plugin.video.example/?m=1": {
                "kind": "movie", "id": "plugin://plugin.video.example/?m=1",
                "name": "Old", "art": null}},
            "watchlist": {"movie:plugin://plugin.video.example/?m=1": 5}
        }"#;
    fs::write(&path, old).unwrap();
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.watchlist()[0].name, "Old");
    assert_eq!(store.watchlist()[0].year, None);
    store
        .put_bookmark(
            "plugin://plugin.video.example/?m=2",
            TitleKind::Movie,
            "New",
            None,
        )
        .unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("\"year\""));
    assert!(!text.contains("\"imdb_id\""));
    assert!(!text.contains('\n'), "the file is written compact");
    let _ = fs::remove_file(&path);
}

#[test]
fn browsing_a_movie_remembers_add_on_facts() {
    let mut store = temp_store("observe-facts");
    let movie = "plugin://plugin.video.example/?action=play&id=7";
    let item = BrowseItem {
        id: movie.into(),
        label: "Example".into(),
        path: movie.into(),
        kind: ItemKind::Play,
        art: None,
        title_kind: Some(TitleKind::Movie),
        season: None,
        episode: None,
        show_title: None,
        show_path: None,
        settings_addon: None,
        info: Some(portal_api::MediaInfo {
            year: Some(2001),
            imdb_id: Some("tt0000007".into()),
            ..Default::default()
        }),
    };
    store
        .observe("plugin://plugin.video.example/?action=movies", &[item])
        .unwrap();
    let detail = store.title(movie).unwrap();
    assert_eq!(detail.year, Some(2001));
    assert_eq!(detail.imdb_id.as_deref(), Some("tt0000007"));
    let _ = fs::remove_file(&store.path);
}

#[test]
fn history_edit_and_remove_use_the_stored_row() {
    let mut store = temp_store("history-edit");
    let movie = "plugin://plugin.video.example/?movie=1";
    for (name, at) in [("First", 1), ("Second", 2)] {
        store
            .record_progress_at(
                &ProgressReport {
                    id: format!("{movie}&cut={name}"),
                    kind: TitleKind::Movie,
                    name: name.into(),
                    art: None,
                    season: None,
                    episode: None,
                    episode_name: None,
                    file: Some(movie.into()),
                    position: 10.0,
                    duration: 100.0,
                },
                at,
            )
            .unwrap();
    }
    let newest = store.history();
    assert_eq!(newest[0].name, "Second");
    assert_eq!(newest[0].slot, 1);
    assert_eq!(newest[1].slot, 0);
    assert!(store.rename_history(1, "  Second cut  ").unwrap());
    assert!(!store.rename_history(9, "Missing").unwrap());
    let renamed = store.history();
    assert_eq!(renamed[0].name, "Second cut");
    assert_eq!(renamed[1].name, "First");
    assert_eq!(store.clear_history().unwrap(), 2);
    assert!(store.history().is_empty());
    assert_eq!(store.clear_history().unwrap(), 0);
    for (name, at) in [("Again", 3), ("Later", 4)] {
        store
            .record_progress_at(
                &ProgressReport {
                    id: format!("{movie}&cut={name}"),
                    kind: TitleKind::Movie,
                    name: name.into(),
                    art: None,
                    season: None,
                    episode: None,
                    episode_name: None,
                    file: Some(movie.into()),
                    position: 10.0,
                    duration: 100.0,
                },
                at,
            )
            .unwrap();
    }
    assert!(store.delete_history(0).unwrap());
    assert!(!store.delete_history(4).unwrap());
    let left = store.history();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].name, "Later");
    assert_eq!(left[0].slot, 0);
    let _ = fs::remove_file(&store.path);
}

#[test]
fn passing_ninety_percent_marks_watched_and_continue_offers_the_next_episode() {
    let mut store = temp_store("ninety");
    let show = "plugin://plugin.video.example/?show=1";
    store
        .observe(show, &[episode(show, 1, 1), episode(show, 1, 2)])
        .unwrap();
    store
        .record_progress_at(
            &ProgressReport {
                id: show.into(),
                kind: TitleKind::Tv,
                name: "Example".into(),
                art: None,
                season: Some(1),
                episode: Some(1),
                episode_name: Some("S1E1 Name".into()),
                file: Some("plugin://example/play/1/1".into()),
                position: 90.0,
                duration: 100.0,
            },
            20,
        )
        .unwrap();
    let cards = store.home_continue();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].episode, Some(2));
    assert!(cards[0].resume_seconds.is_none());
    let detail = store.title(show).unwrap();
    assert!(detail.seasons[0].episodes[0].finished);
    assert_eq!(detail.next.unwrap().episode, 2);
    let history = store.history();
    assert_eq!(history.len(), 1);
    assert!(history[0].finished);
    let _ = fs::remove_file(&store.path);
}

#[test]
fn resume_is_kept_past_sixty_seconds_and_a_tie_prefers_the_later_episode() {
    let mut store = temp_store("resume");
    let show = "plugin://plugin.video.example/?show=1";
    store
        .observe(show, &[episode(show, 1, 1), episode(show, 1, 4)])
        .unwrap();
    for (episode_no, position) in [(1, 80.0), (4, 90.0)] {
        store
            .record_progress_at(
                &ProgressReport {
                    id: show.into(),
                    kind: TitleKind::Tv,
                    name: "Example".into(),
                    art: None,
                    season: Some(1),
                    episode: Some(episode_no),
                    episode_name: None,
                    file: Some(format!("plugin://example/play/1/{episode_no}")),
                    position,
                    duration: 1000.0,
                },
                5,
            )
            .unwrap();
    }
    let cards = store.home_continue();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].episode, Some(4));
    assert_eq!(cards[0].resume_seconds, Some(90.0));
    let _ = fs::remove_file(&store.path);
}

#[test]
fn season_folders_attach_episodes_to_the_show() {
    let mut store = temp_store("seasons");
    let show = "plugin://plugin.video.example/?show=1";
    let season = "plugin://plugin.video.example/?show=1&season=1";
    store
        .observe(
            show,
            &[BrowseItem {
                id: season.into(),
                label: "Season 1".into(),
                path: season.into(),
                kind: ItemKind::Open,
                art: None,
                title_kind: None,
                season: None,
                episode: None,
                show_title: Some("Example".into()),
                show_path: None,
                settings_addon: None,
                info: None,
            }],
        )
        .unwrap();
    store
        .observe(
            season,
            &[BrowseItem {
                id: "ep".into(),
                label: "Pilot".into(),
                path: "plugin://example/play/pilot".into(),
                kind: ItemKind::Play,
                art: None,
                title_kind: None,
                season: None,
                episode: Some(1),
                show_title: None,
                show_path: None,
                settings_addon: None,
                info: None,
            }],
        )
        .unwrap();
    let detail = store.title(show).unwrap();
    assert_eq!(detail.name, "Example");
    assert_eq!(detail.seasons.len(), 1);
    assert_eq!(
        detail.seasons[0].episodes[0].path,
        "plugin://example/play/pilot"
    );
    assert_eq!(
        detail.season_folders,
        vec![SeasonFolder {
            season: 1,
            path: season.into()
        }]
    );
    let _ = fs::remove_file(&store.path);
}

#[test]
fn a_finale_leads_to_the_next_season_once_its_folder_is_listed() {
    let mut store = temp_store("next-season");
    let show = "plugin://plugin.video.example/?show=3";
    let folder = |season: u32| format!("{show}&season={season}");
    let seasons: Vec<BrowseItem> = [0, 1, 2, 3]
        .into_iter()
        .map(|number| {
            let mut open = episode(show, 1, 1);
            open.label = if number == 0 {
                "Specials".into()
            } else {
                format!("Season {number}")
            };
            open.path = folder(number);
            open.kind = ItemKind::Open;
            open.season = None;
            open.episode = None;
            open
        })
        .collect();
    store.observe(show, &seasons).unwrap();
    let mut finale = episode(show, 1, 2);
    finale.show_path = None;
    let mut pilot = episode(show, 1, 1);
    pilot.show_path = None;
    store.observe(&folder(1), &[pilot, finale]).unwrap();
    // Only season 1 is known: nothing follows its finale yet.
    assert!(store
        .next_after(TitleKind::Tv, show, Some(1), Some(2))
        .is_none());
    assert_eq!(store.season_folder_after(show, 1), Some(folder(2)));
    assert_eq!(store.season_folder_after(show, 3), None);
    let mut opener = episode(show, 2, 1);
    opener.show_path = None;
    opener.path = "plugin://example/play/s2e1".into();
    store.observe(&folder(2), &[opener]).unwrap();
    let next = store
        .next_after(TitleKind::Tv, show, Some(1), Some(2))
        .unwrap();
    assert_eq!((next.season, next.episode), (Some(2), Some(1)));
    assert_eq!(next.file, "plugin://example/play/s2e1");
    let _ = fs::remove_file(&store.path);
}

#[test]
fn a_linked_season_folder_does_not_become_a_show() {
    let mut store = temp_store("linked-season");
    let show = "plugin://plugin.video.example/?show=2";
    let season = "plugin://plugin.video.example/?show=2&season=1";
    let mut open = episode(show, 1, 1);
    open.label = "Season 1".into();
    open.path = season.into();
    open.kind = ItemKind::Open;
    open.season = None;
    open.episode = None;
    store.observe(show, &[open]).unwrap();
    let mut first = episode(show, 1, 1);
    first.show_path = None;
    let mut second = episode(show, 1, 2);
    second.show_path = None;
    store.observe(season, &[first, second]).unwrap();
    assert!(store.title(season).is_none());
    assert!(!store
        .doc
        .episodes
        .contains_key(&title_key(TitleKind::Tv, season)));
    assert_eq!(store.doc.titles.len(), 1);
    assert_eq!(store.title(show).unwrap().seasons[0].episodes.len(), 2);
    let _ = fs::remove_file(&store.path);
}

#[test]
fn pruning_drops_only_old_unused_records() {
    let day = 24 * 60 * 60 * 1000;
    let now = 400 * day;
    let mut store = temp_store("prune");
    let kept = "plugin://plugin.video.example/kept";
    let old = "plugin://plugin.video.example/old";
    let fresh = "plugin://plugin.video.example/fresh";
    let listed = "plugin://plugin.video.example/listed";
    store
        .record_progress_at(&resumable_movie(kept, "Kept"), 1)
        .unwrap();
    store.doc.history.clear();
    store
        .doc
        .bookmarks
        .insert(title_key(TitleKind::Movie, kept), 1);
    store
        .record_progress_at(&resumable_movie(old, "Old"), 1)
        .unwrap();
    store
        .record_progress_at(&resumable_movie(fresh, "Fresh"), now - day)
        .unwrap();
    store.doc.history.clear();
    store.remember_title(TitleKind::Movie, listed, "Listed", None);
    store
        .doc
        .watchlist
        .insert(title_key(TitleKind::Movie, listed), 1);
    for record in store.doc.titles.values_mut() {
        record.seen = 1;
    }
    let show = "plugin://plugin.video.example/?show=3";
    let season = "plugin://plugin.video.example/?show=3&season=1";
    store.remember_title(TitleKind::Tv, show, "Show", None);
    store.upsert_episode(
        show,
        EpisodeRecord {
            season: 1,
            episode: 1,
            name: "Pilot".into(),
            path: "plugin://example/play/1/1".into(),
            art: None,
        },
    );
    store.doc.season_folders.insert(
        season.into(),
        SeasonLink {
            show_id: show.into(),
            season: 1,
            show_name: "Show".into(),
        },
    );
    store
        .doc
        .titles
        .get_mut(&title_key(TitleKind::Tv, show))
        .unwrap()
        .seen = 1;
    let recent = "plugin://plugin.video.example/recent";
    store.remember_title(TitleKind::Movie, recent, "Recent", None);
    store
        .doc
        .titles
        .get_mut(&title_key(TitleKind::Movie, recent))
        .unwrap()
        .seen = now;
    store.prune(now);
    assert!(store
        .doc
        .progress
        .contains_key(&title_key(TitleKind::Movie, kept)));
    assert!(!store
        .doc
        .progress
        .contains_key(&title_key(TitleKind::Movie, old)));
    assert!(store
        .doc
        .progress
        .contains_key(&title_key(TitleKind::Movie, fresh)));
    assert!(store.title(kept).is_some());
    assert!(store.title(old).is_none());
    assert!(store.title(fresh).is_some());
    assert!(store.title(listed).is_some());
    assert!(store.title(recent).is_some());
    assert!(store.title(show).is_none());
    assert!(store.doc.episodes.is_empty());
    assert!(store.doc.season_folders.is_empty());
    assert_eq!(store.doc.watchlist.len(), 1);
    assert_eq!(store.doc.bookmarks.len(), 1);
    let _ = fs::remove_file(&store.path);
}

#[test]
fn pruning_caps_unused_titles_keeping_the_newest() {
    let mut store = temp_store("prune-cap");
    let now = 1_000_000;
    for index in 0..UNUSED_TITLE_LIMIT + 2 {
        let id = format!("plugin://plugin.video.example/{index}");
        store.remember_title(TitleKind::Movie, &id, "Movie", None);
        store
            .doc
            .titles
            .get_mut(&title_key(TitleKind::Movie, &id))
            .unwrap()
            .seen = now - index as i64;
    }
    store.prune(now);
    assert_eq!(store.doc.titles.len(), UNUSED_TITLE_LIMIT);
    assert!(store.title("plugin://plugin.video.example/0").is_some());
    assert!(store
        .title(&format!(
            "plugin://plugin.video.example/{}",
            UNUSED_TITLE_LIMIT + 1
        ))
        .is_none());
    let _ = fs::remove_file(&store.path);
}

#[test]
fn episode_labels_parse() {
    assert_eq!(parse_episode_label("S01E02 Hello"), Some((1, 2)));
    assert_eq!(parse_episode_label("1x03 Hello"), Some((1, 3)));
    assert_eq!(parse_episode_label("Friends S01E02"), Some((1, 2)));
    assert_eq!(parse_episode_label("Episode s2 e5"), Some((2, 5)));
    assert_eq!(parse_episode_label("99999999999x1 then 2x04"), Some((2, 4)));
    assert_eq!(season_number("Season 12"), Some(12));
    assert_eq!(season_number("Season 0"), Some(0));
    assert_eq!(season_number("Specials"), Some(0));
    assert_eq!(parse_episode_label("S00E01 Pilot"), Some((0, 1)));
    assert_eq!(parse_episode_label("Movies"), None);
}

#[test]
fn a_special_keeps_its_own_resume_point() {
    let mut store = temp_store("special-progress");
    let show = "plugin://plugin.video.example/?show=specials";
    let mut report = episode_report(show, 1, 120.0);
    report.season = Some(0);
    report.file = Some("plugin://plugin.video.example/?ep=s00e01".into());
    store.record_progress_at(&report, 1).unwrap();
    let key = crate::progress_key(TitleKind::Tv, show, 0, 1);
    assert!(store.doc.progress.contains_key(&key));
    assert!(!store
        .doc
        .progress
        .contains_key(&crate::title_key(TitleKind::Tv, show)));
    let cards = store.home_continue();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].season, Some(0));
    assert_eq!(cards[0].episode, Some(1));
    assert_eq!(
        cards[0].file.as_deref(),
        Some("plugin://plugin.video.example/?ep=s00e01")
    );
    let _ = std::fs::remove_file(&store.path);
}

#[test]
fn a_finished_special_leaves_the_show_where_it_was() {
    let mut store = temp_store("special-aside");
    let show = "plugin://plugin.video.example/?show=aside";
    let mut regular = episode_report(show, 5, 990.0);
    regular.season = Some(3);
    store.record_progress_at(&regular, 1).unwrap();
    let mut special = episode_report(show, 1, 990.0);
    special.season = Some(0);
    store.record_progress_at(&special, 2).unwrap();
    let latest = store.latest_episode(show).unwrap();
    assert_eq!((latest.played.season, latest.played.episode), (3, 5));
    // A special still playing is where the show picks up.
    special.position = 120.0;
    special.episode = Some(2);
    store.record_progress_at(&special, 3).unwrap();
    let latest = store.latest_episode(show).unwrap();
    assert_eq!((latest.played.season, latest.played.episode), (0, 2));
    let _ = std::fs::remove_file(&store.path);
}

#[test]
fn pruning_keeps_an_old_watched_mark() {
    let mut store = temp_store("prune-watched");
    let movie = "plugin://plugin.video.example/?movie=seen";
    let mut report = resumable_movie(movie, "Seen");
    report.position = 990.0;
    store.record_progress_at(&report, 1).unwrap();
    store.clear_history().unwrap();
    store.prune(now_ms());
    assert!(store
        .doc
        .progress
        .contains_key(&title_key(TitleKind::Movie, movie)));
    let _ = fs::remove_file(&store.path);
}

fn resumable_movie(id: &str, name: &str) -> ProgressReport {
    ProgressReport {
        id: id.into(),
        kind: TitleKind::Movie,
        name: name.into(),
        art: None,
        season: None,
        episode: None,
        episode_name: None,
        file: Some(id.into()),
        position: 120.0,
        duration: 1000.0,
    }
}

#[test]
fn continue_hides_titles_removed_from_history() {
    let mut store = temp_store("continue-history");
    let movie = "plugin://plugin.video.example/?movie=1";
    let other = "plugin://plugin.video.example/?movie=2";
    store
        .record_progress_at(&resumable_movie(movie, "One"), 1)
        .unwrap();
    store
        .record_progress_at(&resumable_movie(other, "Two"), 2)
        .unwrap();
    let ids: Vec<_> = store
        .home_continue()
        .into_iter()
        .map(|card| card.id)
        .collect();
    assert!(ids.contains(&movie.to_string()));
    assert!(ids.contains(&other.to_string()));

    assert!(store.delete_history(0).unwrap());
    let left: Vec<_> = store
        .home_continue()
        .into_iter()
        .map(|card| card.id)
        .collect();
    assert!(!left.iter().any(|id| id == movie));
    assert_eq!(left, vec![other.to_string()]);

    // Another title in between gives the replay a row of its own.
    let third = "plugin://plugin.video.example/?movie=3";
    store
        .record_progress_at(&resumable_movie(third, "Three"), 3)
        .unwrap();
    store
        .record_progress_at(&resumable_movie(other, "Two again"), 4)
        .unwrap();
    // The replay replaced Two's older row, so only Three's goes here.
    assert!(store.delete_history(0).unwrap());
    let still: Vec<_> = store
        .home_continue()
        .into_iter()
        .map(|card| card.id)
        .collect();
    assert_eq!(still, vec![other.to_string()]);

    assert_eq!(store.clear_history().unwrap(), 1);
    assert!(store.home_continue().is_empty());
    assert!(store
        .doc
        .progress
        .contains_key(&title_key(TitleKind::Movie, other)));
    let _ = fs::remove_file(&store.path);
}

#[test]
fn continue_drops_an_episode_once_its_history_row_is_gone() {
    let mut store = temp_store("continue-episode");
    let show = "plugin://plugin.video.example/?show=9";
    store
        .observe(
            show,
            &[
                episode(show, 1, 1),
                episode(show, 1, 2),
                episode(show, 1, 4),
            ],
        )
        .unwrap();
    for (episode_no, at) in [(1, 1_i64), (4, 2)] {
        store
            .record_progress_at(
                &ProgressReport {
                    id: show.into(),
                    kind: TitleKind::Tv,
                    name: "Example".into(),
                    art: None,
                    season: Some(1),
                    episode: Some(episode_no),
                    episode_name: None,
                    file: Some(format!("plugin://example/play/1/{episode_no}")),
                    position: 80.0,
                    duration: 1000.0,
                },
                at,
            )
            .unwrap();
    }
    assert_eq!(store.home_continue()[0].episode, Some(4));
    assert!(store.delete_history(1).unwrap());
    let cards = store.home_continue();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, show);
    assert_eq!(cards[0].episode, Some(1));
    assert!(store.delete_history(0).unwrap());
    assert!(store.home_continue().is_empty());

    store
        .record_progress_at(
            &ProgressReport {
                id: show.into(),
                kind: TitleKind::Tv,
                name: "Example".into(),
                art: None,
                season: Some(1),
                episode: Some(1),
                episode_name: Some("S1E1 Name".into()),
                file: Some("plugin://example/play/1/1".into()),
                position: 900.0,
                duration: 1000.0,
            },
            3,
        )
        .unwrap();
    let next = store.home_continue();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].episode, Some(2));
    assert!(next[0].resume_seconds.is_none());
    assert!(store.delete_history(0).unwrap());
    assert!(store.home_continue().is_empty());
    assert!(store
        .doc
        .progress
        .contains_key(&progress_key(TitleKind::Tv, show, 1, 4)));
    let _ = fs::remove_file(&store.path);
}

#[test]
fn usable_title_name_rejects_art_blobs() {
    assert!(usable_title_name("24"));
    assert!(usable_title_name(" 90 Day Fiancé "));
    assert!(!usable_title_name(""));
    assert!(!usable_title_name("   "));
    assert!(!usable_title_name(
        r#"{"poster": "https://image.tmdb.org/t/p/w780/x.jpg"}"#
    ));
    assert!(!usable_title_name(
        "%7B%22poster%22%3A+%22https%3A//image.tmdb.org/t/p/w780/x.jpg%22%7D"
    ));
    assert!(!usable_title_name("Show %22poster%22 still art"));
    assert!(!usable_title_name("%7b%22fanart%22%3Aposter%22"));
    assert!(!usable_title_name("plugin://plugin.video.example/?movie=1"));
    assert!(!usable_title_name("plugin.video.example"));
    assert!(!usable_title_name("https://image.tmdb.org/x.jpg"));
}

#[test]
fn remember_title_refuses_an_art_blob() {
    let mut store = temp_store("art-name");
    let blob = "%7B%22poster%22%3A+%22https%3A//image.tmdb.org/t/p/w780/x.jpg%22%7D";
    let show = "plugin://plugin.video.example/?action=seasons&tvshowtitle=24&year=2001";
    store.remember_title(TitleKind::Tv, show, blob, None);
    assert_eq!(store.title(show).unwrap().name, "24");
    store.remember_title(
        TitleKind::Tv,
        show,
        blob,
        Some("https://image.tmdb.org/x.jpg".into()),
    );
    let kept = store.title(show).unwrap();
    assert_eq!(kept.name, "24");
    assert_eq!(kept.art.as_deref(), Some("https://image.tmdb.org/x.jpg"));

    let bare = "plugin://plugin.video.example/?action=seasons";
    store.remember_title(TitleKind::Tv, bare, blob, None);
    assert_eq!(store.title(bare).unwrap().name, "Title");

    let segmented = "plugin://plugin.video.example/Heat";
    store.remember_title(TitleKind::Movie, segmented, blob, None);
    assert_eq!(store.title(segmented).unwrap().name, "Heat");

    store
        .record_progress_at(
            &ProgressReport {
                id: show.into(),
                kind: TitleKind::Tv,
                name: blob.into(),
                art: None,
                season: Some(1),
                episode: Some(5),
                episode_name: Some("(S01E05) : 4:00 A.M.-5:00 A.M.".into()),
                file: Some("plugin://example/play/1/5".into()),
                position: 80.0,
                duration: 1000.0,
            },
            9,
        )
        .unwrap();
    assert_eq!(store.title(show).unwrap().name, "24");
    assert_eq!(store.history()[0].name, "24");
    assert_eq!(store.home_continue()[0].name, "24");
    let _ = fs::remove_file(&store.path);
}

#[test]
fn cards_and_history_hide_a_stored_art_blob() {
    let mut store = temp_store("art-card");
    let blob = r#"{"poster":"https://image.tmdb.org/t/p/w780/x.jpg","fanart":"https://image.tmdb.org/y.jpg"}"#;
    let show = "plugin://plugin.video.example/?action=seasons&tvshowtitle=24";
    store
        .record_progress_at(
            &ProgressReport {
                id: show.into(),
                kind: TitleKind::Tv,
                name: "Kept".into(),
                art: None,
                season: Some(1),
                episode: Some(5),
                episode_name: Some("(S01E05) : 4:00 A.M.-5:00 A.M.".into()),
                file: Some("plugin://example/play/1/5".into()),
                position: 120.0,
                duration: 1000.0,
            },
            4,
        )
        .unwrap();
    store
        .put_bookmark(show, TitleKind::Tv, "Kept", None)
        .unwrap();
    store
        .doc
        .titles
        .get_mut(&title_key(TitleKind::Tv, show))
        .unwrap()
        .name = blob.to_string();
    assert_eq!(store.home_continue()[0].name, "Kept");
    assert_eq!(store.bookmarks()[0].name, "Kept");
    assert_eq!(store.history()[0].name, "Kept");

    store.doc.history[0].name = blob.to_string();
    assert_eq!(store.history()[0].name, "24");
    assert_eq!(store.home_continue()[0].name, "24");
    assert_eq!(store.title(show).unwrap().name, "24");
    let _ = fs::remove_file(&store.path);
}

#[test]
fn progress_without_history_never_continues() {
    let mut store = temp_store("orphan-progress");
    let movie = "plugin://plugin.video.example/?movie=orphan";
    store.remember_title(TitleKind::Movie, movie, "Orphan", None);
    store.doc.progress.insert(
        title_key(TitleKind::Movie, movie),
        ProgressRecord {
            position: 120.0,
            duration: 1000.0,
            at: 9,
            watched: false,
        },
    );
    assert!(store.history().is_empty());
    assert!(store.home_continue().is_empty());
    store
        .record_progress_at(&resumable_movie(movie, "Orphan"), 10)
        .unwrap();
    assert_eq!(store.home_continue().len(), 1);
    assert_eq!(store.clear_history().unwrap(), 1);
    assert!(store.home_continue().is_empty());
    assert!(store
        .doc
        .progress
        .contains_key(&title_key(TitleKind::Movie, movie)));
    let _ = fs::remove_file(&store.path);
}

#[test]
fn finished_movie_does_not_continue_and_exact_thresholds_hold() {
    let mut store = temp_store("finished-movie");
    let movie = "plugin://plugin.video.example/?movie=done";
    store
        .record_progress_at(
            &ProgressReport {
                id: movie.into(),
                kind: TitleKind::Movie,
                name: "Done".into(),
                art: None,
                season: None,
                episode: None,
                episode_name: None,
                file: Some(movie.into()),
                position: 90.0,
                duration: 100.0,
            },
            1,
        )
        .unwrap();
    assert!(store.home_continue().is_empty());
    assert!(store.history()[0].finished);
    assert!(store.title(movie).unwrap().watched);
    assert!(store.title(movie).unwrap().resume_seconds.is_none());

    let early = "plugin://plugin.video.example/?movie=early";
    store
        .record_progress_at(
            &ProgressReport {
                id: early.into(),
                kind: TitleKind::Movie,
                name: "Early".into(),
                art: None,
                season: None,
                episode: None,
                episode_name: None,
                file: Some(early.into()),
                position: 60.0,
                duration: 1000.0,
            },
            2,
        )
        .unwrap();
    assert!(store.home_continue().is_empty());
    assert!(store.title(early).unwrap().resume_seconds.is_none());
    let _ = fs::remove_file(&store.path);
}

#[test]
fn watchlist_and_bookmarks_edit_remove_and_refuse_bad_names() {
    let mut store = temp_store("lists-edit");
    let show = "plugin://plugin.video.example/?show=lists";
    let movie = "plugin://plugin.video.example/?movie=lists";
    let blob = "%7B%22poster%22%3A+%22https%3A//image.tmdb.org/t/p/w780/x.jpg%22%7D";

    store
        .put_watchlist(show, TitleKind::Tv, "First", None)
        .unwrap();
    store
        .put_watchlist(show, TitleKind::Tv, "Renamed", None)
        .unwrap();
    assert_eq!(store.watchlist().len(), 1);
    assert_eq!(store.watchlist()[0].name, "Renamed");

    store
        .put_bookmark(movie, TitleKind::Movie, "", None)
        .unwrap();
    assert_eq!(store.bookmarks()[0].name, "Title");
    store
        .put_bookmark(movie, TitleKind::Movie, blob, None)
        .unwrap();
    assert_eq!(store.bookmarks().len(), 1);
    assert_eq!(store.bookmarks()[0].name, "Title");
    store
        .put_bookmark(movie, TitleKind::Movie, movie, None)
        .unwrap();
    assert_eq!(store.bookmarks()[0].name, "Title");

    store
        .put_watchlist("", TitleKind::Movie, "Nope", None)
        .unwrap();
    store.delete_watchlist("").unwrap();
    assert_eq!(store.watchlist().len(), 1);

    store.delete_watchlist(show).unwrap();
    assert!(store.watchlist().is_empty());
    assert_eq!(store.bookmarks().len(), 1);
    store.delete_bookmark(movie).unwrap();
    assert!(store.bookmarks().is_empty());
    store.delete_bookmark(movie).unwrap();
    store.delete_watchlist("missing").unwrap();
    let _ = fs::remove_file(&store.path);
}

#[test]
fn history_rename_refuses_empty_and_scrubs_blobs() {
    let mut store = temp_store("history-scrub");
    let movie = "plugin://plugin.video.example/?movie=scrub";
    store
        .record_progress_at(&resumable_movie(movie, "Heat"), 1)
        .unwrap();
    assert!(!store.rename_history(0, "").unwrap());
    assert!(!store.rename_history(0, "   ").unwrap());
    assert_eq!(store.history()[0].name, "Heat");
    assert!(store
        .rename_history(0, r#"{"poster":"https://image.tmdb.org/t/p/w780/x.jpg"}"#)
        .unwrap());
    assert_eq!(store.history()[0].name, "Heat");
    assert!(store.rename_history(0, "  Heat cut  ").unwrap());
    assert_eq!(store.history()[0].name, "Heat cut");
    assert_eq!(store.title(movie).unwrap().name, "Heat cut");
    let _ = fs::remove_file(&store.path);
}

#[test]
fn a_shifted_history_slot_finds_its_title() {
    let mut store = temp_store("history-slot");
    let a = "plugin://plugin.video.example/?movie=a";
    let b = "plugin://plugin.video.example/?movie=b";
    store
        .record_progress_at(&resumable_movie(a, "A"), 1)
        .unwrap();
    store
        .record_progress_at(&resumable_movie(b, "B"), 2)
        .unwrap();
    // The page showed B at slot 1, then the oldest row went away.
    assert_eq!(store.history_slot(1, b, None, None), Some(1));
    assert!(store.delete_history(0).unwrap());
    assert_eq!(store.history_slot(1, b, None, None), Some(0));
    assert_eq!(store.history_slot(0, a, None, None), None);
    let _ = fs::remove_file(&store.path);
}

#[test]
fn duplicate_history_keys_still_filter_continue_per_row() {
    let mut store = temp_store("dup-history");
    let movie = "plugin://plugin.video.example/?movie=dup";
    store
        .record_progress_at(&resumable_movie(movie, "One"), 1)
        .unwrap();
    let between = "plugin://plugin.video.example/?movie=between";
    store
        .record_progress_at(&resumable_movie(between, "Between"), 2)
        .unwrap();
    store
        .record_progress_at(&resumable_movie(movie, "Two"), 3)
        .unwrap();
    // A replay moves the title's one row to the top.
    assert_eq!(store.history().len(), 2);
    assert!(store.delete_history(1).unwrap());
    assert_eq!(store.home_continue().len(), 1);
    assert_eq!(store.history().len(), 1);
    assert!(store.delete_history(0).unwrap());
    assert!(store.home_continue().is_empty());
    assert!(store
        .doc
        .progress
        .contains_key(&title_key(TitleKind::Movie, movie)));
    let _ = fs::remove_file(&store.path);
}

#[test]
fn one_episode_under_two_addresses_is_one_row_and_one_card() {
    let mut store = temp_store("two-addresses");
    let from_seasons = "plugin://plugin.video.example/?action=seasons&show=7";
    let from_episodes = "plugin://plugin.video.example/?action=episodes&show=7";
    store
        .record_progress_at(&episode_report(from_seasons, 2, 300.0), 1)
        .unwrap();
    store
        .record_progress_at(&episode_report(from_episodes, 2, 320.0), 2)
        .unwrap();
    assert_eq!(store.history().len(), 1);
    // The card offers the newest place, not the first address's.
    let cards = store.home_continue();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, from_episodes);
    assert_eq!(cards[0].resume_seconds, Some(320.0));
    let _ = fs::remove_file(&store.path);
}

#[test]
fn same_named_titles_that_differ_keep_their_own_rows() {
    let mut store = temp_store("same-names");
    // Two shows whose episodes share a title: the show names differ.
    let mut first = episode_report("plugin://example/?show=a", 1, 300.0);
    first.name = "Show A".into();
    first.episode_name = Some("Pilot".into());
    let mut second = episode_report("plugin://example/?show=b", 1, 400.0);
    second.name = "Show B".into();
    second.episode_name = Some("Pilot".into());
    store.record_progress_at(&first, 1).unwrap();
    store.record_progress_at(&second, 2).unwrap();
    // Two films that share a title stay two rows.
    store
        .record_progress_at(&resumable_movie("plugin://example/?movie=1", "Dune"), 3)
        .unwrap();
    store
        .record_progress_at(&resumable_movie("plugin://example/?movie=2", "Dune"), 4)
        .unwrap();
    assert_eq!(store.history().len(), 4);
    let shows = store
        .home_continue()
        .into_iter()
        .filter(|card| card.kind == TitleKind::Tv)
        .count();
    assert_eq!(shows, 2);
    let _ = fs::remove_file(&store.path);
}

fn episode_report(show: &str, episode: u32, position: f64) -> ProgressReport {
    ProgressReport {
        id: show.into(),
        kind: TitleKind::Tv,
        name: "Show".into(),
        art: None,
        season: Some(1),
        episode: Some(episode),
        episode_name: None,
        file: None,
        position,
        duration: 1000.0,
    }
}

#[test]
fn repeated_progress_updates_the_newest_history_row() {
    let mut store = temp_store("history-coalesce");
    let show = "plugin://plugin.video.example/?show=long";
    let movie = "plugin://plugin.video.example/?movie=long";
    for (at, position) in [(1, 100.0), (2, 108.0), (3, 116.0)] {
        store
            .record_progress_at(&episode_report(show, 1, position), at)
            .unwrap();
    }
    let rows = store.history();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].position, 116.0);
    assert_eq!(rows[0].at, 3);

    store
        .record_progress_at(&episode_report(show, 2, 50.0), 4)
        .unwrap();
    store
        .record_progress_at(&resumable_movie(movie, "Long"), 5)
        .unwrap();
    store
        .record_progress_at(&resumable_movie(movie, "Long"), 6)
        .unwrap();
    store
        .record_progress_at(&episode_report(show, 2, 60.0), 7)
        .unwrap();
    // Each title keeps one row, at its latest play.
    let rows = store.history();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].episode, Some(2));
    assert_eq!(rows[0].at, 7);
    assert_eq!(rows[1].id, movie);
    assert_eq!(rows[1].at, 6);
    let _ = fs::remove_file(&store.path);
}

#[test]
fn a_steady_playhead_does_not_rewrite_the_library_every_tick() {
    let path =
        std::env::temp_dir().join(format!("portal-library-quiet-{}.json", std::process::id()));
    let _ = fs::remove_file(&path);
    let movie = "plugin://plugin.video.example/?movie=quiet";
    {
        let mut store = Store::open(&path).unwrap();
        store
            .record_progress_at(&resumable_movie(movie, "Quiet"), 1_000)
            .unwrap();
        let written = fs::read(&path).unwrap();
        let mut nudged = resumable_movie(movie, "Quiet");
        nudged.position = 130.0;
        store.apply_progress(&nudged, 1_000 + 8_000, false).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            written,
            "a few seconds later does not rewrite the file"
        );
        assert_eq!(store.history()[0].position, 130.0);
        nudged.position = 20.0;
        store.apply_progress(&nudged, 1_000 + 9_000, false).unwrap();
        assert_ne!(fs::read(&path).unwrap(), written, "a seek back is written");
        let after_seek = fs::read(&path).unwrap();
        nudged.position = 24.0;
        store.apply_progress(&nudged, 1_000 + 10_000, true).unwrap();
        assert_ne!(
            fs::read(&path).unwrap(),
            after_seek,
            "leaving the page writes the playhead"
        );
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.history()[0].position, 24.0);
    let _ = fs::remove_file(&path);
}

#[test]
fn a_corrupt_library_file_is_set_aside_and_the_library_starts_empty() {
    let name = format!("portal-library-corrupt-{}", std::process::id());
    let dir = std::env::temp_dir();
    let path = dir.join(format!("{name}.json"));
    fs::write(&path, "{\"titles\": {").unwrap();
    let store = Store::open(&path).unwrap();
    assert!(store.history().is_empty());
    assert!(!path.exists());
    let aside: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let file = entry.file_name().to_string_lossy().into_owned();
            file.starts_with(&format!("{name}.corrupt-")) && file.ends_with(".json")
        })
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(
        fs::read_to_string(aside[0].path()).unwrap(),
        "{\"titles\": {"
    );
    let _ = fs::remove_file(aside[0].path());
}

#[test]
fn browsing_the_same_folder_again_does_not_rewrite_the_file() {
    let mut store = temp_store("observe-unchanged");
    let show = "plugin://example/show";
    let items = vec![episode(show, 1, 1), episode(show, 1, 2)];
    store.observe(show, &items).unwrap();
    assert!(store.path.exists());
    fs::remove_file(&store.path).unwrap();
    store.observe(show, &items).unwrap();
    assert!(!store.path.exists(), "nothing changed, nothing saved");

    let mut renamed = items.clone();
    renamed[1].label = "S1E2 New name".into();
    store.observe(show, &renamed).unwrap();
    assert!(store.path.exists());
    let _ = fs::remove_file(&store.path);
}

#[test]
fn a_position_that_is_not_a_number_does_not_spoil_the_file() {
    let path = std::env::temp_dir().join(format!(
        "portal-library-not-a-number-{}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    let movie = "plugin://plugin.video.example/?movie=nan";
    {
        let mut store = Store::open(&path).unwrap();
        store
            .put_watchlist(movie, TitleKind::Movie, "Example", None)
            .unwrap();
        let mut report = resumable_movie(movie, "Example");
        report.position = f64::NAN;
        report.duration = f64::INFINITY;
        store.record_progress_at(&report, 1).unwrap();
    }
    // The file still reads, so nothing was set aside.
    let store = Store::open(&path).unwrap();
    assert_eq!(store.watchlist().len(), 1);
    assert_eq!(store.history().len(), 1);
    assert_eq!(store.history()[0].position, 0.0);
    assert!(store.home_continue().is_empty());
    let _ = fs::remove_file(&path);
}

#[test]
fn a_clock_that_ran_ahead_does_not_hold_a_show_back() {
    let mut store = temp_store("clock-ahead");
    let show = "plugin://plugin.video.example/?show=clock";
    store
        .observe(show, &[episode(show, 1, 3), episode(show, 1, 4)])
        .unwrap();
    // Episode 3 played while the clock was ahead; episode 4 after it was fixed.
    store
        .record_progress_at(&episode_report(show, 3, 120.0), 10_000)
        .unwrap();
    store
        .record_progress_at(&episode_report(show, 4, 120.0), 5_000)
        .unwrap();
    let latest = store.latest_episode(show).unwrap();
    assert_eq!((latest.played.season, latest.played.episode), (1, 4));
    let cards = store.home_continue();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].episode, Some(4));
    // History keeps the time the clock gave.
    assert_eq!(store.history()[0].at, 5_000);
    let _ = fs::remove_file(&store.path);
}

fn alpha() -> Source {
    Source {
        addon_id: Some("plugin.video.alpha".into()),
        name: Some("Alpha".into()),
        path: Some("plugin://plugin.video.alpha/".into()),
    }
}

fn play_of(
    kind: PlayKind,
    id: &str,
    file: &str,
    title: &str,
    season: Option<u32>,
    episode: Option<u32>,
) -> PlayRequest {
    PlayRequest {
        kind,
        id: id.into(),
        file: file.into(),
        title: title.into(),
        quality: Default::default(),
        season,
        episode,
        art: None,
        show_title: Some(title.into()),
        show_id: Some(id.into()),
        resume_seconds: None,
        manual_links: false,
        audio_language: None,
    }
}

fn filed(show: &str, season: u32, number: u32, path: &str) -> BrowseItem {
    let mut item = episode(show, season, number);
    item.label = format!("{season}x{number:02}. Episode");
    item.path = path.into();
    item.show_title = Some("The Mentalist".into());
    item.show_path = Some(show.into());
    item
}

#[test]
fn an_episode_plays_on_the_chosen_source_when_it_has_a_copy() {
    let mut store = temp_store("route-mentalist");
    let delta = "plugin://plugin.video.delta/?action=seasons&tvshowtitle=The+Mentalist&year=";
    let episodes = "plugin://plugin.video.alpha/?action=episodes&tvshowtitle=The+Mentalist&year=";
    let seasons = "plugin://plugin.video.alpha/?action=seasons&tvshowtitle=The+Mentalist&year=";
    let delta_file = "plugin://plugin.video.delta/?action=play&episode=6x9";
    let thin_file = "plugin://plugin.video.alpha/?action=play&episode=6x9&list=episodes";
    let fat_file = "plugin://plugin.video.alpha/?action=play&episode=6x9&list=seasons";
    store
        .observe(delta, &[filed(delta, 6, 9, delta_file)])
        .unwrap();
    store
        .observe(
            episodes,
            &[
                filed(
                    episodes,
                    6,
                    8,
                    "plugin://plugin.video.alpha/?action=play&episode=6x8",
                ),
                filed(episodes, 6, 9, thin_file),
                filed(
                    episodes,
                    6,
                    10,
                    "plugin://plugin.video.alpha/?action=play&episode=6x10",
                ),
            ],
        )
        .unwrap();
    let fat: Vec<_> = (1..=5)
        .map(|episode| {
            filed(
                seasons,
                6,
                episode,
                &format!("plugin://plugin.video.alpha/?action=play&episode=6x{episode}"),
            )
        })
        .chain(std::iter::once(filed(seasons, 6, 9, fat_file)))
        .collect();
    // 6x9 is filed twice in this list; upsert keeps one row, plus 6x1-5 except the replaced 6x9.
    store.observe(seasons, &fat).unwrap();
    assert!(store.episode_len(seasons) > store.episode_len(episodes));
    store.set_source(alpha()).unwrap();

    let routed = store.route_play(&play_of(
        PlayKind::Episode,
        delta,
        delta_file,
        "The Mentalist",
        Some(6),
        Some(9),
    ));
    assert_eq!(
        routed,
        PlayRoute::Retarget {
            id: seasons.into(),
            file: fat_file.into(),
            show_id: Some(seasons.into()),
        }
    );

    // The episodes folder is the same add-on and has fewer episodes.
    let thin = store.route_play(&play_of(
        PlayKind::Episode,
        episodes,
        thin_file,
        "The Mentalist",
        Some(6),
        Some(9),
    ));
    assert_eq!(
        thin,
        PlayRoute::Retarget {
            id: seasons.into(),
            file: fat_file.into(),
            show_id: Some(seasons.into()),
        }
    );

    // Equal counts stay put, so two addresses of one episode remain two plays.
    let twin_a = "plugin://plugin.video.alpha/?action=folder&show=a";
    let twin_b = "plugin://plugin.video.alpha/?action=folder&show=b";
    store
        .observe(
            twin_a,
            &[filed(twin_a, 1, 1, "plugin://plugin.video.alpha/?play=a")],
        )
        .unwrap();
    store
        .observe(
            twin_b,
            &[filed(twin_b, 1, 1, "plugin://plugin.video.alpha/?play=b")],
        )
        .unwrap();
    assert_eq!(
        store.route_play(&play_of(
            PlayKind::Episode,
            twin_a,
            "plugin://plugin.video.alpha/?play=a",
            "The Mentalist",
            Some(1),
            Some(1),
        )),
        PlayRoute::Keep
    );

    // No Alpha copy of this episode: do not open Delta.
    let missing = store.route_play(&play_of(
        PlayKind::Episode,
        delta,
        delta_file,
        "The Mentalist",
        Some(7),
        Some(1),
    ));
    assert_eq!(
        missing,
        PlayRoute::Missing {
            addon: "Alpha".into()
        }
    );

    // A different year is a different title.
    let delta_movie = "plugin://plugin.video.delta/?action=play&title=The+Mentalist&year=2011";
    let other_year = "plugin://plugin.video.alpha/?action=play&title=The+Mentalist&year=2008";
    let same_year = "plugin://plugin.video.alpha/?action=play&title=The+Mentalist&year=2011";
    store
        .observe(
            "plugin://plugin.video.alpha/",
            &[movie_item(other_year, "The Mentalist (2008)")],
        )
        .unwrap();
    assert_eq!(
        store.route_play(&play_of(
            PlayKind::Movie,
            delta_movie,
            delta_movie,
            "The Mentalist (2011)",
            None,
            None,
        )),
        PlayRoute::Missing {
            addon: "Alpha".into()
        }
    );
    store
        .observe(
            "plugin://plugin.video.alpha/",
            &[movie_item(same_year, "The Mentalist (2011)")],
        )
        .unwrap();
    assert_eq!(
        store.route_play(&play_of(
            PlayKind::Movie,
            delta_movie,
            delta_movie,
            "The Mentalist (2011)",
            None,
            None,
        )),
        PlayRoute::Retarget {
            id: same_year.into(),
            file: same_year.into(),
            show_id: None,
        }
    );

    let song = play_of(
        PlayKind::Song,
        "plugin://plugin.video.delta/?song=1",
        "plugin://plugin.video.delta/?song=1",
        "Theme",
        None,
        None,
    );
    assert_eq!(store.route_play(&song), PlayRoute::Keep);
    let mut local = play_of(
        PlayKind::Episode,
        delta,
        "C:/videos/episode.mkv",
        "The Mentalist",
        Some(6),
        Some(9),
    );
    local.show_title = Some("The Mentalist".into());
    assert_eq!(store.route_play(&local), PlayRoute::Keep);
    store
        .set_source(Source {
            addon_id: None,
            name: None,
            path: None,
        })
        .unwrap();
    assert_eq!(
        store.route_play(&play_of(
            PlayKind::Episode,
            delta,
            delta_file,
            "The Mentalist",
            Some(6),
            Some(9),
        )),
        PlayRoute::Keep
    );
    let _ = fs::remove_file(&store.path);
}

fn movie_item(path: &str, label: &str) -> BrowseItem {
    BrowseItem {
        id: path.into(),
        label: label.into(),
        path: path.into(),
        kind: ItemKind::Play,
        art: None,
        title_kind: Some(TitleKind::Movie),
        season: None,
        episode: None,
        show_title: None,
        show_path: None,
        settings_addon: None,
        info: None,
    }
}

#[test]
fn an_episodes_folder_joins_the_seasons_show_without_renaming_it_year() {
    let mut store = temp_store("listing-mentalist");
    let seasons = "plugin://plugin.video.alpha/?action=seasons&tvshowtitle=The+Mentalist&year=";
    let episodes = "plugin://plugin.video.alpha/?action=episodes&tvshowtitle=The+Mentalist&year=";
    store
        .observe(
            seasons,
            &[filed(
                seasons,
                6,
                9,
                "plugin://plugin.video.alpha/?play=s6e9",
            )],
        )
        .unwrap();
    assert_eq!(folder_label(episodes), "year");
    let mut extra = filed(episodes, 6, 10, "plugin://plugin.video.alpha/?play=s6e10");
    extra.show_path = None;
    extra.show_title = Some("The Mentalist".into());
    store.observe(episodes, &[extra]).unwrap();
    assert!(store.title(episodes).is_none());
    let show = store.title(seasons).unwrap();
    assert_eq!(show.name, "The Mentalist");
    assert!(show
        .seasons
        .iter()
        .flat_map(|season| &season.episodes)
        .any(|episode| episode.path == "plugin://plugin.video.alpha/?play=s6e10"));
    assert!(store
        .doc
        .titles
        .values()
        .all(|record| record.name != "year"));

    // A seasons folder stays itself even when a thinner listing exists.
    let mut season_row = filed(seasons, 7, 1, "plugin://plugin.video.alpha/?play=s7e1");
    season_row.show_path = None;
    store.observe(seasons, &[season_row]).unwrap();
    assert!(store.title(seasons).unwrap().seasons.iter().any(|season| {
        season
            .episodes
            .iter()
            .any(|episode| episode.path == "plugin://plugin.video.alpha/?play=s7e1")
    }));
    let _ = fs::remove_file(&store.path);
}
