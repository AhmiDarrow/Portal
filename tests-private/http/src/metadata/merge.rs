use super::*;
use portal_engine::kodi_info::SOURCE_KODI;

fn link(label: &str, url: &str) -> InfoLink {
    InfoLink {
        label: label.into(),
        url: url.into(),
    }
}

#[test]
fn a_seed_is_cleaned() {
    let seed: MediaInfo = serde_json::from_value(serde_json::json!({
            "title": "  Pilot ",
            "show_title": "Example Show",
            "season": 1,
            "episode": 2,
            "year": 20,
            "premiered": "2008-01-20 00:00",
            "rating": 0,
            "votes": 0,
            "genres": ["Drama", " ", "Drama"],
            "imdb_id": "https://www.imdb.com/title/tt0959621/",
            "tmdb_id": "abc",
            "tvdb_id": " 81189 ",
            "trailer": "plugin://plugin.video.example/trailer",
            "links": [{"label": "Bad", "url": "javascript:alert(1)"}, {"label": "Ok", "url": "https://example.org/"}]
        }))
        .unwrap();
    let seed = normalize_seed(seed);
    assert_eq!(seed.kind, Some(MediaKind::Episode));
    assert_eq!(seed.title.as_deref(), Some("Pilot"));
    assert_eq!(seed.year, None);
    assert_eq!(seed.premiered.as_deref(), Some("2008-01-20"));
    assert_eq!(seed.rating, None);
    assert_eq!(seed.votes, None);
    assert_eq!(seed.genres, vec!["Drama"]);
    assert_eq!(seed.imdb_id.as_deref(), Some("tt0959621"));
    assert_eq!(seed.tmdb_id, None);
    assert_eq!(seed.tvdb_id.as_deref(), Some("81189"));
    assert_eq!(seed.trailer, None);
    assert_eq!(seed.links, vec![link("Ok", "https://example.org/")]);
}

fn seed() -> MediaInfo {
    MediaInfo {
        kind: Some(MediaKind::Movie),
        title: Some("Example Movie".into()),
        year: Some(1994),
        imdb_id: Some("tt0111161".into()),
        plot: Some("The add-on's plot.".into()),
        poster: Some("image://poster.jpg/".into()),
        genres: vec!["Add-on genre".into()],
        sources: vec![SOURCE_KODI.into()],
        ..MediaInfo::default()
    }
}

fn tmdb() -> MediaInfo {
    MediaInfo {
        kind: Some(MediaKind::Tv),
        title: Some("Example Movie (TMDB)".into()),
        original_title: Some("Exemple".into()),
        year: Some(1995),
        imdb_id: Some("tt9999999".into()),
        tmdb_id: Some("278".into()),
        plot: Some("TMDB's plot.".into()),
        rating: Some(8.7),
        votes: Some(100),
        genres: vec!["Drama".into()],
        cast: vec![CastMember {
            name: "Actor".into(),
            role: None,
            thumb: None,
        }],
        poster: Some("https://image.tmdb.org/t/p/w500/p.jpg".into()),
        sources: vec![SOURCE_TMDB.into()],
        links: vec![link("TMDB", "https://www.themoviedb.org/movie/278")],
        ..MediaInfo::default()
    }
}

fn wiki() -> MediaInfo {
    MediaInfo {
        plot: Some("Wikipedia's lead.".into()),
        poster: Some("https://upload.wikimedia.org/p.jpg".into()),
        tagline: Some("Only here".into()),
        sources: vec![SOURCE_WIKIPEDIA.into()],
        links: vec![
            link("Wikipedia", "https://en.wikipedia.org/wiki/Example"),
            link("IMDb", "https://www.imdb.com/title/tt0111161/"),
        ],
        ..MediaInfo::default()
    }
}

#[test]
fn identity_comes_from_the_seed_and_details_from_the_richest_source() {
    let merged = merge(seed(), vec![wiki(), tmdb()]);
    // Identity: the seed's.
    assert_eq!(merged.kind, Some(MediaKind::Movie));
    assert_eq!(merged.title.as_deref(), Some("Example Movie"));
    assert_eq!(merged.year, Some(1994));
    assert_eq!(merged.imdb_id.as_deref(), Some("tt0111161"));
    // Filled from online where the seed had none.
    assert_eq!(merged.tmdb_id.as_deref(), Some("278"));
    // Descriptive: TMDB first.
    assert_eq!(merged.plot.as_deref(), Some("TMDB's plot."));
    assert_eq!(merged.genres, vec!["Drama"]);
    assert_eq!(merged.rating, Some(8.7));
    assert_eq!(merged.votes, Some(100));
    assert_eq!(merged.original_title.as_deref(), Some("Exemple"));
    // Artwork: the seed's, then TMDB's before Wikipedia's.
    assert_eq!(merged.poster.as_deref(), Some("image://poster.jpg/"));
    let bare = MediaInfo {
        poster: None,
        ..seed()
    };
    assert_eq!(
        merge(bare, vec![wiki(), tmdb()]).poster.as_deref(),
        Some("https://image.tmdb.org/t/p/w500/p.jpg")
    );
    // Wikipedia only fills what nobody else had.
    assert_eq!(merged.tagline.as_deref(), Some("Only here"));
    assert_eq!(
        merged.sources,
        vec![SOURCE_KODI, SOURCE_TMDB, SOURCE_WIKIPEDIA]
    );
    assert_eq!(
        merged.links,
        vec![
            imdb_link("tt0111161"),
            link("TMDB", "https://www.themoviedb.org/movie/278"),
            link("Wikipedia", "https://en.wikipedia.org/wiki/Example"),
        ]
    );
}

#[test]
fn the_seed_beats_wikipedia_for_plot_and_poster() {
    let merged = merge(seed(), vec![wiki()]);
    assert_eq!(merged.plot.as_deref(), Some("The add-on's plot."));
    assert_eq!(merged.poster.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(merged.genres, vec!["Add-on genre"]);

    let bare = MediaInfo {
        title: Some("Example".into()),
        ..MediaInfo::default()
    };
    let merged = merge(bare, vec![wiki()]);
    assert_eq!(merged.plot.as_deref(), Some("Wikipedia's lead."));
    assert_eq!(merged.sources, vec![SOURCE_WIKIPEDIA]);
}

#[test]
fn a_richer_show_source_wins_and_nothing_found_keeps_the_seed() {
    let thin = MediaInfo {
        plot: Some("Thin.".into()),
        sources: vec![SOURCE_TMDB.into()],
        ..MediaInfo::default()
    };
    let rich = MediaInfo {
        plot: Some("Rich.".into()),
        genres: vec!["Drama".into()],
        runtime_minutes: Some(45),
        sources: vec![SOURCE_TVMAZE.into()],
        ..MediaInfo::default()
    };
    let merged = merge(MediaInfo::default(), vec![thin, rich]);
    assert_eq!(merged.plot.as_deref(), Some("Rich."));
    assert_eq!(merged.sources, vec![SOURCE_TVMAZE, SOURCE_TMDB]);

    let merged = merge(seed(), Vec::new());
    assert_eq!(merged.plot.as_deref(), Some("The add-on's plot."));
    assert_eq!(merged.links, vec![imdb_link("tt0111161")]);
    assert_eq!(merged.sources, vec![SOURCE_KODI]);
}

#[test]
fn a_seed_from_the_page_keeps_kodi_art_and_its_own_source_only() {
    let seed: MediaInfo = serde_json::from_value(serde_json::json!({
        "kind": "tv",
        "title": "Example Show",
        "genres": null,
        "cast": null,
        "sources": ["Kodi add-on", "TMDB", "TVmaze"],
        "links": [{"label": "TVmaze", "url": "https://www.tvmaze.com/shows/1/example"}],
        "poster": "image://poster.jpg/",
        "fanart": "image://fanart.jpg/"
    }))
    .unwrap();
    let seed = normalize_seed(seed);
    assert_eq!(seed.kind, Some(MediaKind::Tv));
    assert_eq!(seed.sources, vec![SOURCE_KODI]);

    let merged = merge(seed.clone(), Vec::new());
    assert_eq!(merged.sources, vec![SOURCE_KODI]);
    assert_eq!(merged.poster.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(merged.links.len(), 1);

    // Online artwork never replaces Kodi's.
    let bare = MediaInfo {
        plot: Some("Plot.".into()),
        sources: vec![SOURCE_TVMAZE.into()],
        links: vec![link("TVmaze", "https://www.tvmaze.com/shows/1/example")],
        ..MediaInfo::default()
    };
    let merged = merge(seed.clone(), vec![bare]);
    assert_eq!(merged.poster.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(merged.fanart.as_deref(), Some("image://fanart.jpg/"));
    assert_eq!(merged.sources, vec![SOURCE_KODI, SOURCE_TVMAZE]);
    assert_eq!(merged.links.len(), 1);
    let merged = merge(seed, vec![tmdb()]);
    assert_eq!(merged.poster.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(merged.fanart.as_deref(), Some("image://fanart.jpg/"));
    assert_eq!(merged.cast.len(), 1);
}

#[test]
fn an_imdb_link_appears_for_an_online_id() {
    let seed = MediaInfo {
        title: Some("Example".into()),
        ..MediaInfo::default()
    };
    let found = MediaInfo {
        imdb_id: Some("tt0903747".into()),
        sources: vec![SOURCE_TVMAZE.into()],
        ..MediaInfo::default()
    };
    let merged = merge(seed, vec![found]);
    assert_eq!(merged.links, vec![imdb_link("tt0903747")]);
}
