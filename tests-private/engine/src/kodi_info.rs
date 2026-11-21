use super::*;
use serde_json::json;

/// What Kodi returns for a folder row the add-on set no info on.
fn bare_folder() -> Value {
    json!({
        "filetype": "directory",
        "file": "plugin://plugin.video.example/?action=movies",
        "label": "Movies",
        "type": "unknown",
        "title": "",
        "plot": "",
        "plotoutline": "",
        "tagline": "",
        "year": 0,
        "premiered": "",
        "rating": 0.0,
        "votes": "",
        "genre": [],
        "mpaa": "",
        "runtime": 0,
        "studio": [],
        "director": [],
        "writer": [],
        "cast": [],
        "imdbnumber": "",
        "uniqueid": {},
        "originaltitle": "",
        "trailer": "",
        "showtitle": "",
        "season": -1,
        "episode": -1,
        "country": [],
        "duration": 0,
        "thumbnail": "image://folder.png/",
        "art": {"thumb": "image://folder.png/"}
    })
}

#[test]
fn a_folder_without_labels_has_no_info() {
    assert_eq!(media_info(&bare_folder()), None);
    let mut titled = bare_folder();
    titled["title"] = json!("Movies");
    titled["season"] = json!(2);
    assert_eq!(media_info(&titled), None);
    assert_eq!(media_info(&json!("plugin://x")), None);
}

#[test]
fn a_menu_folder_that_repeats_its_name_has_no_info() {
    let mut menu = bare_folder();
    menu["label"] = json!("Discover Movies");
    menu["title"] = json!("Discover Movies");
    menu["plot"] = json!("Discover Movies");
    menu["plotoutline"] = json!(" discover movies ");
    menu["votes"] = json!("0");
    assert_eq!(media_info(&menu), None);
    menu["plotoutline"] = json!("Films people are watching.");
    assert_eq!(
        media_info(&menu).and_then(|info| info.plot).as_deref(),
        Some("Films people are watching.")
    );
}

#[test]
fn a_movie_maps_every_label() {
    let item = json!({
        "filetype": "file",
        "file": "plugin://plugin.video.example/?action=play&title=Example&year=1994",
        "label": "[B]Example Movie[/B]",
        "type": "movie",
        "title": "Example Movie",
        "originaltitle": "Exemple",
        "plot": "Line one.[CR]Line [I]two[/I].",
        "plotoutline": "Short.",
        "tagline": "A tagline",
        "year": 1994,
        "premiered": "1994-09-23",
        "rating": 8.699999809265137,
        "votes": "2,712,345",
        "genre": ["Drama", "Crime / Drama", ""],
        "mpaa": "Rated R",
        "runtime": 8520,
        "duration": 0,
        "studio": ["Studio A"],
        "director": ["Director A"],
        "writer": ["Writer A", "Writer B"],
        "country": ["United States of America"],
        "cast": [
            {"name": "Second", "role": "", "order": 1, "thumbnail": ""},
            {"name": "First", "role": "Lead", "order": 0, "thumbnail": "image://cast.jpg/"}
        ],
        "imdbnumber": "tt0111161",
        "uniqueid": {"imdb": "tt0111161", "tmdb": "278", "tvdb": ""},
        "trailer": "https://www.youtube.com/watch?v=abc",
        "showtitle": "",
        "season": -1,
        "episode": -1,
        "thumbnail": "image://thumb.jpg/",
        "art": {"poster": "image://poster.jpg/", "fanart": "image://fanart.jpg/"}
    });
    let info = media_info(&item).unwrap();
    assert_eq!(info.kind, Some(MediaKind::Movie));
    assert_eq!(info.title.as_deref(), Some("Example Movie"));
    assert_eq!(info.original_title.as_deref(), Some("Exemple"));
    assert_eq!(info.plot.as_deref(), Some("Line one.\nLine two."));
    assert_eq!(info.tagline.as_deref(), Some("A tagline"));
    assert_eq!(info.year, Some(1994));
    assert_eq!(info.premiered.as_deref(), Some("1994-09-23"));
    assert_eq!(info.rating, Some(8.7));
    assert_eq!(info.votes, Some(2_712_345));
    assert_eq!(info.genres, vec!["Drama", "Crime"]);
    assert_eq!(info.mpaa.as_deref(), Some("R"));
    assert_eq!(info.runtime_minutes, Some(142));
    assert_eq!(info.studios, vec!["Studio A"]);
    assert_eq!(info.writers, vec!["Writer A", "Writer B"]);
    assert_eq!(info.countries, vec!["United States of America"]);
    assert_eq!(info.cast.len(), 2);
    assert_eq!(info.cast[0].name, "First");
    assert_eq!(info.cast[0].role.as_deref(), Some("Lead"));
    assert_eq!(info.cast[0].thumb.as_deref(), Some("image://cast.jpg/"));
    assert_eq!(info.cast[1].role, None);
    assert_eq!(info.cast[1].thumb, None);
    assert_eq!(info.imdb_id.as_deref(), Some("tt0111161"));
    assert_eq!(info.tmdb_id.as_deref(), Some("278"));
    assert_eq!(info.tvdb_id, None);
    assert_eq!(
        info.trailer.as_deref(),
        Some("https://www.youtube.com/watch?v=abc")
    );
    assert_eq!(info.poster.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(info.fanart.as_deref(), Some("image://fanart.jpg/"));
    assert_eq!(info.season, None);
    assert_eq!(info.episode, None);
    assert_eq!(info.sources, vec![SOURCE_KODI]);
    assert_eq!(info.links, vec![imdb_link("tt0111161")]);
}

#[test]
fn an_episode_keeps_numbers_and_drops_plugin_trailers() {
    let item = json!({
        "type": "episode",
        "title": "Pilot",
        "showtitle": "Example Show",
        "season": 1,
        "episode": 1,
        "plot": "",
        "plotoutline": "Outline only.",
        "year": 0,
        "premiered": "2008-01-20",
        "rating": 0,
        "votes": "0",
        "runtime": 0,
        "duration": 3480,
        "imdbnumber": "81189",
        "uniqueid": {"tvdb": "349232", "imdb": "https://www.imdb.com/title/tt0959621/"},
        "trailer": "plugin://plugin.video.example/play/?video_id=abc",
        "cast": [],
        "thumbnail": "image://still.jpg/",
        "art": {"thumb": "image://still.jpg/", "tvshow.poster": "image://show.jpg/"}
    });
    let info = media_info(&item).unwrap();
    assert_eq!(info.kind, Some(MediaKind::Episode));
    assert_eq!(info.show_title.as_deref(), Some("Example Show"));
    assert_eq!((info.season, info.episode), (Some(1), Some(1)));
    assert_eq!(info.plot.as_deref(), Some("Outline only."));
    assert_eq!(info.year, Some(2008));
    assert_eq!(info.rating, None);
    assert_eq!(info.votes, None);
    assert_eq!(info.runtime_minutes, Some(58));
    // A numeric imdbnumber is the show's default id, not an IMDb id.
    assert_eq!(info.imdb_id.as_deref(), Some("tt0959621"));
    assert_eq!(info.tvdb_id.as_deref(), Some("349232"));
    assert_eq!(info.trailer, None);
    assert_eq!(info.poster.as_deref(), Some("image://show.jpg/"));
}

#[test]
fn specials_keep_season_zero_and_cast_is_capped() {
    let cast: Vec<Value> = (0..30)
        .map(|index| json!({"name": format!("Actor {index}"), "role": "", "order": index}))
        .collect();
    let item = json!({
        "type": "episode",
        "season": 0,
        "episode": 3,
        "cast": cast,
        "year": 3000,
        "premiered": "not a date",
        "imdbnumber": "",
        "uniqueid": {"tmdb": "abc"}
    });
    let info = media_info(&item).unwrap();
    assert_eq!(info.season, Some(0));
    assert_eq!(info.cast.len(), CAST_LIMIT);
    assert_eq!(info.cast[0].name, "Actor 0");
    assert_eq!(info.year, None);
    assert_eq!(info.premiered, None);
    assert_eq!(info.tmdb_id, None);
}

#[test]
fn helpers_filter_bad_values() {
    assert_eq!(seconds_to_minutes(0), None);
    assert_eq!(seconds_to_minutes(20), Some(1));
    assert_eq!(seconds_to_minutes(5400), Some(90));
    assert_eq!(rating(0.0), None);
    assert_eq!(rating(11.0), None);
    assert_eq!(rating(7.25), Some(7.3));
    assert_eq!(
        date_prefix("2008-01-20T00:00:00"),
        Some("2008-01-20".into())
    );
    assert_eq!(date_prefix("2008-13-20"), None);
    assert_eq!(date_prefix("0000-00-00"), None);
    assert!(is_http("http://example.com/a"));
    assert!(!is_http("plugin://x/"));
}
