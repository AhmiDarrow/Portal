use super::*;

fn mark(position: f64, duration: f64, at: i64) -> ProgressMark {
    ProgressMark {
        position,
        duration,
        at,
        watched: finished(position, duration),
    }
}

#[test]
fn ninety_percent_is_watched_and_sixty_seconds_is_not_a_resume() {
    assert!(!finished(89.0, 100.0));
    assert!(finished(90.0, 100.0));
    assert!(finished(95.0, 100.0));
    assert!(!finished(10.0, 0.0));
    assert!(!finished(f64::NAN, 100.0));
    assert!(!finished(50.0, f64::INFINITY));
    assert!(!resumable(60.0, 1000.0));
    assert!(resumable(61.0, 1000.0));
    assert!(resumable(89.0, 100.0));
    assert!(!resumable(90.0, 100.0));
    assert!(!resumable(950.0, 1000.0));
    assert!(!resumable(f64::NAN, 1000.0));
    assert!(!resumable(120.0, f64::NAN));
}

#[test]
fn plugin_paths_keep_their_colons() {
    let show = "plugin://plugin.video.example/?show=1";
    let key = progress_key(TitleKind::Tv, show, 2, 3);
    match parse_key(&key).unwrap() {
        ParsedKey::Episode {
            id,
            season,
            episode,
        } => {
            assert_eq!(id, show);
            assert_eq!((season, episode), (2, 3));
        }
        other => panic!("unexpected {other:?}"),
    }
    match parse_key(&title_key(TitleKind::Tv, show)).unwrap() {
        ParsedKey::Show { id } => assert_eq!(id, show),
        other => panic!("unexpected {other:?}"),
    }
    match parse_key(&title_key(TitleKind::Movie, show)).unwrap() {
        ParsedKey::Movie { id } => assert_eq!(id, show),
        other => panic!("unexpected {other:?}"),
    }
    let special = progress_key(TitleKind::Tv, show, 0, 1);
    match parse_key(&special).unwrap() {
        ParsedKey::Episode {
            id,
            season,
            episode,
        } => {
            assert_eq!(id, show);
            assert_eq!((season, episode), (0, 1));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        progress_key(TitleKind::Tv, show, 0, 0),
        title_key(TitleKind::Tv, show)
    );
    assert_eq!(
        progress_key(TitleKind::Tv, show, 2, 0),
        title_key(TitleKind::Tv, show)
    );
}

#[test]
fn play_starts_on_a_regular_season_when_specials_come_first() {
    let seasons = [
        SeasonCount {
            number: 0,
            episodes: 2,
        },
        SeasonCount {
            number: 1,
            episodes: 3,
        },
    ];
    assert_eq!(
        next_episode(&seasons, None),
        Some(EpisodeNum {
            season: 1,
            episode: 1
        })
    );
    let only_specials = [SeasonCount {
        number: 0,
        episodes: 2,
    }];
    assert_eq!(
        next_episode(&only_specials, None),
        Some(EpisodeNum {
            season: 0,
            episode: 1
        })
    );
    let finished_special = LastPlayed {
        season: 0,
        episode: 2,
        watched: true,
    };
    assert_eq!(
        next_episode(&seasons, Some(finished_special)),
        Some(EpisodeNum {
            season: 1,
            episode: 1
        })
    );
}

#[test]
fn next_episode_rolls_forward_and_stops_at_the_finale() {
    let seasons = [
        SeasonCount {
            number: 1,
            episodes: 2,
        },
        SeasonCount {
            number: 2,
            episodes: 1,
        },
    ];
    assert_eq!(
        next_episode(&seasons, None),
        Some(EpisodeNum {
            season: 1,
            episode: 1
        })
    );
    let partial = LastPlayed {
        season: 1,
        episode: 1,
        watched: false,
    };
    assert_eq!(
        next_episode(&seasons, Some(partial)),
        Some(EpisodeNum {
            season: 1,
            episode: 1
        })
    );
    let done = LastPlayed {
        season: 1,
        episode: 1,
        watched: true,
    };
    assert_eq!(
        next_episode(&seasons, Some(done)),
        Some(EpisodeNum {
            season: 1,
            episode: 2
        })
    );
    let end_of_season = LastPlayed {
        season: 1,
        episode: 2,
        watched: true,
    };
    assert_eq!(
        next_episode(&seasons, Some(end_of_season)),
        Some(EpisodeNum {
            season: 2,
            episode: 1
        })
    );
    let finale = LastPlayed {
        season: 2,
        episode: 1,
        watched: true,
    };
    assert_eq!(next_episode(&seasons, Some(finale)), None);
    let missing = LastPlayed {
        season: 9,
        episode: 1,
        watched: true,
    };
    assert_eq!(next_episode(&seasons, Some(missing)), None);
    assert_eq!(next_episode(&[], None), None);
}

#[test]
fn continue_watching_is_one_card_and_the_later_episode_wins_a_tie() {
    let show = "plugin://plugin.video.example/?show=1";
    let mut progress = BTreeMap::new();
    progress.insert(
        progress_key(TitleKind::Tv, show, 1, 1),
        mark(100.0, 1000.0, 50),
    );
    progress.insert(
        progress_key(TitleKind::Tv, show, 1, 5),
        mark(120.0, 1000.0, 50),
    );
    let cards = continuing(&progress, |_| {
        vec![SeasonCount {
            number: 1,
            episodes: 5,
        }]
    });
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].episode, 5);
    assert!(cards[0].progress.is_some());
}

#[test]
fn a_finished_episode_continues_on_the_next_one() {
    let show = "plugin://plugin.video.example/?show=1";
    let mut progress = BTreeMap::new();
    progress.insert(
        progress_key(TitleKind::Tv, show, 1, 2),
        mark(90.0, 100.0, 10),
    );
    let cards = continuing(&progress, |_| {
        vec![SeasonCount {
            number: 1,
            episodes: 3,
        }]
    });
    assert_eq!(cards.len(), 1);
    assert_eq!((cards[0].season, cards[0].episode), (1, 3));
    assert!(cards[0].progress.is_none());
}

#[test]
fn a_finished_finale_offers_nothing() {
    let show = "plugin://plugin.video.example/?show=1";
    let mut progress = BTreeMap::new();
    progress.insert(
        progress_key(TitleKind::Tv, show, 1, 2),
        mark(100.0, 100.0, 10),
    );
    let cards = continuing(&progress, |_| {
        vec![SeasonCount {
            number: 1,
            episodes: 2,
        }]
    });
    assert!(cards.is_empty());
}

#[test]
fn a_finished_special_keeps_the_show_on_its_regular_episode() {
    let show = "plugin://plugin.video.example/?show=1";
    let seasons = || {
        vec![
            SeasonCount {
                number: 0,
                episodes: 3,
            },
            SeasonCount {
                number: 3,
                episodes: 10,
            },
        ]
    };
    let mut progress = BTreeMap::new();
    progress.insert(
        progress_key(TitleKind::Tv, show, 3, 5),
        mark(90.0, 100.0, 10),
    );
    progress.insert(
        progress_key(TitleKind::Tv, show, 0, 1),
        mark(90.0, 100.0, 20),
    );
    let cards = continuing(&progress, |_| seasons());
    assert_eq!(cards.len(), 1);
    assert_eq!((cards[0].season, cards[0].episode), (3, 6));
    // A regular episode opened for a moment gives the show no place.
    progress.insert(
        progress_key(TitleKind::Tv, show, 3, 5),
        mark(5.0, 100.0, 10),
    );
    let cards = continuing(&progress, |_| seasons());
    assert_eq!((cards[0].season, cards[0].episode), (0, 2));
    // With only specials watched, the specials carry on.
    progress.remove(&progress_key(TitleKind::Tv, show, 3, 5));
    let cards = continuing(&progress, |_| seasons());
    assert_eq!((cards[0].season, cards[0].episode), (0, 2));
}
