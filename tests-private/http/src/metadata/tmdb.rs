use super::*;
use serde_json::json;

fn query(kind: Option<MediaKind>) -> Query {
    Query {
        kind,
        ..Query::default()
    }
}

#[test]
fn keys_are_told_apart_by_shape() {
    assert_eq!(
        key_kind("0123456789abcdef0123456789ABCDEF"),
        KeyKind::ApiKey
    );
    assert_eq!(
        key_kind(
            "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiIwMTIzNDU2Nzg5YWJjZGVmIn0.c2lnbmF0dXJlc2lnbmF0dXJl"
        ),
        KeyKind::Bearer
    );
    assert!(plausible_key("0123456789abcdef0123456789abcdef"));
    assert!(!plausible_key("has a space in it 0123456789"));
    assert!(!plausible_key("short"));
}

#[test]
fn find_answers_pick_the_asked_kind() {
    let found = json!({
        "movie_results": [{"id": 278}],
        "tv_results": [{"id": 1396}],
        "tv_episode_results": [{"id": 62085, "show_id": 1396, "season_number": 1, "episode_number": 1}]
    });
    assert_eq!(
        pick_found(&found, &query(Some(MediaKind::Movie))),
        Some(Target::Movie(278))
    );
    assert_eq!(
        pick_found(&found, &query(Some(MediaKind::Tv))),
        Some(Target::Tv(1396))
    );
    assert_eq!(pick_found(&found, &query(None)), Some(Target::Movie(278)));

    let episode_only = json!({"movie_results": [], "tv_results": [], "tv_episode_results": [
        {"id": 62085, "show_id": 1396, "season_number": 1, "episode_number": 1}
    ]});
    assert_eq!(
        pick_found(&episode_only, &query(Some(MediaKind::Episode))),
        Some(Target::Episode {
            show: 1396,
            season: 1,
            episode: 1
        })
    );
    // The page's numbers win over the episode the id belongs to.
    let asked = Query {
        kind: Some(MediaKind::Episode),
        season: Some(2),
        episode: Some(3),
        ..Query::default()
    };
    assert_eq!(
        pick_found(&found, &asked),
        Some(Target::Episode {
            show: 1396,
            season: 2,
            episode: 3
        })
    );
    assert_eq!(
        pick_found(
            &json!({"movie_results": []}),
            &query(Some(MediaKind::Movie))
        ),
        None
    );
}

#[test]
fn search_takes_a_matching_title_near_the_year() {
    let results = json!({"results": [
        {"id": 1, "title": "Heat Wave", "release_date": "1995-01-01"},
        {"id": 2, "title": "Heat", "release_date": "1986-03-07"},
        {"id": 3, "title": "Heat", "release_date": "1995-12-15"},
        {"id": 4, "title": "Heat", "release_date": ""}
    ]});
    assert_eq!(
        pick_search(&results, "Heat", Some(1995), "release_date"),
        Some(3)
    );
    assert_eq!(
        pick_search(&results, "heat", Some(1996), "release_date"),
        Some(3)
    );
    assert_eq!(pick_search(&results, "Heat", None, "release_date"), Some(2));
    assert_eq!(
        pick_search(&results, "Heat", Some(2010), "release_date"),
        Some(4)
    );
    assert_eq!(pick_search(&results, "Cold", None, "release_date"), None);
    let shows =
        json!({"results": [{"id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20"}]});
    assert_eq!(
        pick_search(&shows, "Breaking Bad", Some(2008), "first_air_date"),
        Some(1396)
    );
}

fn movie_fixture() -> Value {
    json!({
        "id": 278,
        "imdb_id": "tt0111161",
        "title": "The Shawshank Redemption",
        "original_title": "The Shawshank Redemption",
        "overview": "Imprisoned in the 1940s...",
        "tagline": "Fear can hold you prisoner. Hope can set you free.",
        "release_date": "1994-09-23",
        "runtime": 142,
        "vote_average": 8.707,
        "vote_count": 27000,
        "genres": [{"id": 18, "name": "Drama"}, {"id": 80, "name": "Crime"}],
        "production_companies": [{"name": "Castle Rock Entertainment"}],
        "production_countries": [{"iso_3166_1": "US", "name": "United States of America"}],
        "poster_path": "/poster.jpg",
        "backdrop_path": "/backdrop.jpg",
        "credits": {
            "cast": [
                {"name": "Morgan Freeman", "character": "Ellis Boyd 'Red' Redding", "order": 1, "profile_path": "/mf.jpg"},
                {"name": "Tim Robbins", "character": "Andy Dufresne", "order": 0, "profile_path": null}
            ],
            "crew": [
                {"name": "Frank Darabont", "job": "Director", "department": "Directing"},
                {"name": "Frank Darabont", "job": "Screenplay", "department": "Writing"},
                {"name": "Stephen King", "job": "Novel", "department": "Writing"},
                {"name": "Roger Deakins", "job": "Director of Photography", "department": "Camera"}
            ]
        },
        "external_ids": {"imdb_id": "tt0111161"},
        "videos": {"results": [
            {"site": "Vimeo", "type": "Trailer", "key": "v1"},
            {"site": "YouTube", "type": "Teaser", "key": "t1"},
            {"site": "YouTube", "type": "Trailer", "key": "unofficial", "official": false},
            {"site": "YouTube", "type": "Trailer", "key": "PLl99DlL6b4", "official": true}
        ]},
        "release_dates": {"results": [
            {"iso_3166_1": "DE", "release_dates": [{"certification": "12"}]},
            {"iso_3166_1": "US", "release_dates": [{"certification": ""}, {"certification": "R"}]}
        ]}
    })
}

#[test]
fn a_movie_parses() {
    let info = parse_movie(&movie_fixture());
    assert_eq!(info.kind, Some(MediaKind::Movie));
    assert_eq!(info.title.as_deref(), Some("The Shawshank Redemption"));
    assert_eq!(info.year, Some(1994));
    assert_eq!(info.premiered.as_deref(), Some("1994-09-23"));
    assert_eq!(info.runtime_minutes, Some(142));
    assert_eq!(info.rating, Some(8.7));
    assert_eq!(info.votes, Some(27000));
    assert_eq!(info.genres, vec!["Drama", "Crime"]);
    assert_eq!(info.mpaa.as_deref(), Some("R"));
    assert_eq!(info.directors, vec!["Frank Darabont"]);
    assert_eq!(info.writers, vec!["Frank Darabont", "Stephen King"]);
    assert_eq!(info.countries, vec!["United States of America"]);
    assert_eq!(info.cast[0].name, "Tim Robbins");
    assert_eq!(info.cast[0].thumb, None);
    assert_eq!(
        info.cast[1].thumb.as_deref(),
        Some("https://image.tmdb.org/t/p/w185/mf.jpg")
    );
    assert_eq!(info.imdb_id.as_deref(), Some("tt0111161"));
    assert_eq!(info.tmdb_id.as_deref(), Some("278"));
    assert_eq!(
        info.trailer.as_deref(),
        Some("https://www.youtube.com/watch?v=PLl99DlL6b4")
    );
    assert_eq!(
        info.poster.as_deref(),
        Some("https://image.tmdb.org/t/p/w500/poster.jpg")
    );
    assert_eq!(
        info.fanart.as_deref(),
        Some("https://image.tmdb.org/t/p/w1280/backdrop.jpg")
    );
    assert_eq!(info.sources, vec![SOURCE_TMDB]);
    assert_eq!(info.links[0].url, "https://www.themoviedb.org/movie/278");
}

fn show_fixture() -> Value {
    json!({
        "id": 1396,
        "name": "Breaking Bad",
        "original_name": "Breaking Bad",
        "overview": "A chemistry teacher...",
        "first_air_date": "2008-01-20",
        "episode_run_time": [45, 47],
        "vote_average": 8.9,
        "vote_count": 15000,
        "genres": [{"name": "Drama"}, {"name": "Crime"}],
        "networks": [{"name": "AMC"}],
        "production_companies": [{"name": "Sony Pictures Television Studios"}, {"name": "AMC"}],
        "created_by": [{"name": "Vince Gilligan"}],
        "poster_path": "/bb.jpg",
        "backdrop_path": "/bbf.jpg",
        "external_ids": {"imdb_id": "tt0903747", "tvdb_id": 81189},
        "content_ratings": {"results": [{"iso_3166_1": "US", "rating": "TV-MA"}]},
        "credits": {"cast": [{"name": "Bryan Cranston", "character": "Walter White", "order": 0}]}
    })
}

#[test]
fn a_show_parses() {
    let info = parse_tv(&show_fixture());
    assert_eq!(info.kind, Some(MediaKind::Tv));
    assert_eq!(info.title.as_deref(), Some("Breaking Bad"));
    assert_eq!(info.year, Some(2008));
    assert_eq!(info.runtime_minutes, Some(45));
    assert_eq!(info.mpaa.as_deref(), Some("TV-MA"));
    assert_eq!(
        info.studios,
        vec!["AMC", "Sony Pictures Television Studios"]
    );
    assert_eq!(info.writers, vec!["Vince Gilligan"]);
    assert_eq!(info.imdb_id.as_deref(), Some("tt0903747"));
    assert_eq!(info.tvdb_id.as_deref(), Some("81189"));
    assert_eq!(info.tmdb_id.as_deref(), Some("1396"));
    assert_eq!(info.cast[0].role.as_deref(), Some("Walter White"));
    assert_eq!(info.links[0].url, "https://www.themoviedb.org/tv/1396");
}

#[test]
fn an_episode_parses_with_its_show() {
    let episode = json!({
        "id": 62085,
        "name": "Pilot",
        "overview": "Walter White...",
        "air_date": "2008-01-20",
        "season_number": 1,
        "episode_number": 1,
        "runtime": 58,
        "vote_average": 8.2,
        "vote_count": 200,
        "still_path": "/still.jpg",
        "crew": [{"name": "Vince Gilligan", "job": "Director", "department": "Directing"}],
        "credits": {
            "cast": [{"name": "Bryan Cranston", "character": "Walter White", "order": 0}],
            "guest_stars": [{"name": "Guest", "character": "Someone", "order": 500}],
            "crew": [{"name": "Vince Gilligan", "job": "Writer", "department": "Writing"}]
        },
        "external_ids": {"imdb_id": "tt0959621", "tvdb_id": 349232}
    });
    let info = parse_episode(1396, 1, 1, Some(&show_fixture()), Some(&episode));
    assert_eq!(info.kind, Some(MediaKind::Episode));
    assert_eq!(info.title.as_deref(), Some("Pilot"));
    assert_eq!(info.show_title.as_deref(), Some("Breaking Bad"));
    assert_eq!((info.season, info.episode), (Some(1), Some(1)));
    assert_eq!(info.plot.as_deref(), Some("Walter White..."));
    assert_eq!(info.runtime_minutes, Some(58));
    assert_eq!(info.genres, vec!["Drama", "Crime"]);
    assert_eq!(info.writers, vec!["Vince Gilligan"]);
    assert_eq!(info.cast.len(), 2);
    assert_eq!(info.cast[1].name, "Guest");
    assert_eq!(info.imdb_id.as_deref(), Some("tt0959621"));
    assert_eq!(info.tvdb_id.as_deref(), Some("349232"));
    assert_eq!(info.tmdb_id, None);
    assert_eq!(
        info.poster.as_deref(),
        Some("https://image.tmdb.org/t/p/w500/bb.jpg")
    );
    assert_eq!(
        info.fanart.as_deref(),
        Some("https://image.tmdb.org/t/p/w1280/still.jpg")
    );
    assert_eq!(
        info.links[0].url,
        "https://www.themoviedb.org/tv/1396/season/1/episode/1"
    );

    let missing = parse_episode(1396, 9, 9, Some(&show_fixture()), None);
    assert_eq!(missing.plot, None);
    assert_eq!(missing.show_title.as_deref(), Some("Breaking Bad"));
    assert_eq!(missing.links[0].url, "https://www.themoviedb.org/tv/1396");
}
