use super::*;

fn found(id: &str, source: Source, language: &str, release: &str) -> Found {
    Found {
        id: id.into(),
        source,
        language: language.into(),
        release: release.into(),
        hearing_impaired: false,
        downloads: None,
        rating: None,
        hash_match: false,
        episode: None,
        episode_unsure: false,
        machine_translated: false,
    }
}

fn langs(list: &[&str]) -> Vec<String> {
    list.iter().map(|code| code.to_string()).collect()
}

#[test]
fn releases_name_their_group_medium_and_resolution() {
    let release = Release::parse("Show.S01E01.720p.WEB-DL.DD5.1.H.264-GrP");
    assert_eq!(release.group.as_deref(), Some("grp"));
    assert_eq!(release.medium, Some("web"));
    assert_eq!(release.resolution, Some("720p"));
    assert_eq!(
        Release::parse("Movie.2010.1080p.BluRay.x264-[SITE.EX]")
            .group
            .as_deref(),
        Some("site")
    );
    assert_eq!(Release::parse("TAGX").group.as_deref(), Some("tagx"));
    assert_eq!(Release::parse("720p.BluRay.GRPM").group, None);
    assert_eq!(Release::parse("Movie.2010.WEB-DL").group, None);
    assert_eq!(Release::parse("Movie 2010 1080p").group, None);
    assert_eq!(Release::parse("DVDRip.GRPS").medium, Some("dvd"));
    assert_eq!(
        Release::parse("Show.S01E02.1080p.WEB.H264-GRP.mkv")
            .group
            .as_deref(),
        Some("grp")
    );
    assert_eq!(
        Release::parse("Show.S01E01.WEB-DL-GRP[site]")
            .group
            .as_deref(),
        Some("grp")
    );
    assert_eq!(
        Release::parse("Some-Movie.2021.1080p.WEB-DL.x264-GRP.mkv")
            .group
            .as_deref(),
        Some("grp")
    );
}

#[test]
fn a_language_or_hi_tag_after_the_group_is_not_the_group() {
    assert_eq!(
        group_of("Show.S01E02.720p.WEB.x264-GRPA.eng").as_deref(),
        Some("grpa")
    );
    assert_eq!(
        group_of("Show.S01E02.HDTV.x264-GRPB.HI").as_deref(),
        Some("grpb")
    );
    assert_eq!(
        group_of("Show.S01E02.HDTV.x264-GRP.en.srt").as_deref(),
        Some("grp")
    );
}

#[test]
fn a_hyphenated_title_is_not_a_group() {
    assert_eq!(
        Release::parse("Some-Movie.Title.1080p.mkv").group,
        None
    );
    assert_eq!(Release::parse("Some-Movie.mkv").group, None);
    assert_eq!(Release::parse("Some-Title 2000").group, None);
    let items = vec![found(
        "gestdown:1",
        Source::Gestdown,
        "en",
        "Some-Movie.Title.WEB",
    )];
    let languages = langs(&["en"]);
    let prefs = Prefs {
        languages: &languages,
        hearing_impaired: false,
        release: Some("Some-Movie.Title.1080p.mkv"),
    };
    assert_eq!(rank(&items, &prefs)[0].matched, SubtitleMatch::Title);
}

#[test]
fn language_order_comes_first() {
    let items = vec![
        found("gestdown:fr", Source::Gestdown, "fr", "x"),
        found("gestdown:en", Source::Gestdown, "en", "x"),
        found("gestdown:de", Source::Gestdown, "de", "x"),
        found("subdl:br", Source::Subdl, "pt-BR", "x"),
    ];
    let languages = langs(&["en", "fr", "pt-PT"]);
    let prefs = Prefs {
        languages: &languages,
        hearing_impaired: false,
        release: None,
    };
    let ranked = rank(&items, &prefs);
    let ids: Vec<&str> = ranked.iter().map(|item| item.id.as_str()).collect();
    // German is not wanted; Brazilian Portuguese is close to Portuguese.
    assert_eq!(ids, vec!["gestdown:en", "gestdown:fr", "subdl:br"]);
    assert_eq!(ranked[0].language_name, "English");
}

#[test]
fn hash_then_group_then_hearing_then_popularity() {
    let mut hash = found(
        "opensubtitles:1",
        Source::OpenSubtitles,
        "en",
        "Other.Release-ABC",
    );
    hash.hash_match = true;
    let group = found("gestdown:2", Source::Gestdown, "en", "720p.WEB.GrP");
    let mut hearing = found("gestdown:3", Source::Gestdown, "en", "WEB-DL");
    hearing.hearing_impaired = true;
    hearing.downloads = Some(100_000);
    let mut popular = found("gestdown:4", Source::Gestdown, "en", "HDTV.TAGX");
    popular.downloads = Some(5_000);
    let plain = found("gestdown:5", Source::Gestdown, "en", "HDTV.TAGX");
    let items = vec![plain, popular, hearing, group, hash];
    let languages = langs(&["en"]);
    let prefs = Prefs {
        languages: &languages,
        hearing_impaired: false,
        release: Some("Show.S01E01.720p.WEB-DL.DD5.1.H.264-GrP"),
    };
    let ranked = rank(&items, &prefs);
    let ids: Vec<&str> = ranked.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "opensubtitles:1",
            "gestdown:2",
            "gestdown:4",
            "gestdown:5",
            "gestdown:3"
        ]
    );
    assert_eq!(ranked[0].matched, SubtitleMatch::Hash);
    assert_eq!(ranked[1].matched, SubtitleMatch::Release);
    assert_eq!(ranked[2].matched, SubtitleMatch::Title);

    let prefer_hearing = Prefs {
        hearing_impaired: true,
        ..prefs
    };
    let ranked = rank(&items, &prefer_hearing);
    assert_eq!(ranked[2].id, "gestdown:3");
}

#[test]
fn any_of_several_release_names_can_match() {
    let several = found(
        "subdl:a",
        Source::Subdl,
        "en",
        "Movie.1994.720p.BluRay.x264-TAGA
Movie.1994.1080p.BluRay.x264-TAGB",
    );
    let other = found("subdl:b", Source::Subdl, "en", "Movie.1994.DVDRip-TAGC");
    let languages = langs(&["en"]);
    let prefs = Prefs {
        languages: &languages,
        hearing_impaired: false,
        release: Some("Movie.1994.1080p.BluRay.DTS.x264-TAGB"),
    };
    let ranked = rank(&[other, several], &prefs);
    assert_eq!(ranked[0].id, "subdl:a");
    assert_eq!(ranked[0].matched, SubtitleMatch::Release);
    assert_eq!(ranked[0].release, "Movie.1994.720p.BluRay.x264-TAGA");
}

#[test]
fn opensubtitles_breaks_ties_and_duplicates_drop() {
    let items = vec![
        found("subdl:a", Source::Subdl, "en", "Movie"),
        found("opensubtitles:b", Source::OpenSubtitles, "en", "Movie"),
        found("subdl:a", Source::Subdl, "en", "Movie"),
    ];
    let languages = langs(&["en"]);
    let prefs = Prefs {
        languages: &languages,
        hearing_impaired: false,
        release: None,
    };
    let ranked = rank(&items, &prefs);
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].id, "opensubtitles:b");
}

#[test]
fn rows_not_named_for_the_episode_rank_lower() {
    let mut unsure = found("subsource:1", Source::Subsource, "en", "Show.S01.WEB");
    unsure.episode_unsure = true;
    unsure.downloads = Some(50_000);
    let named = found("subdl:a-b", Source::Subdl, "en", "Show.S01E02.WEB");
    let languages = langs(&["en"]);
    let prefs = Prefs {
        languages: &languages,
        hearing_impaired: false,
        release: None,
    };
    let ranked = rank(&[unsure, named], &prefs);
    assert_eq!(ranked[0].id, "subdl:a-b");
    assert_eq!(ranked[0].source, "SubDL");
    assert_eq!(ranked[1].source, "SubSource");
}

#[test]
fn machine_translations_rank_below_human_ones_in_the_same_language() {
    let mut machine = found(
        "subtitlecat:subs/1/Show.S01E02.WEB-GrP-en.srt",
        Source::SubtitleCat,
        "en",
        "Show.S01E02.720p.WEB-GrP",
    );
    machine.machine_translated = true;
    machine.downloads = Some(100_000);
    let mut human = found("tvsubtitles:1", Source::TvSubtitles, "en", "Show 1x02");
    human.hearing_impaired = true;
    let french = found("gestdown:fr", Source::Gestdown, "fr", "Show.S01E02.WEB-GrP");
    let languages = langs(&["en", "fr"]);
    let prefs = Prefs {
        languages: &languages,
        hearing_impaired: false,
        release: Some("Show.S01E02.720p.WEB.H264-GrP"),
    };
    let ranked = rank(&[machine, french, human], &prefs);
    let ids: Vec<&str> = ranked.iter().map(|item| item.id.as_str()).collect();
    // Language order still comes first.
    assert_eq!(
        ids,
        vec![
            "tvsubtitles:1",
            "subtitlecat:subs/1/Show.S01E02.WEB-GrP-en.srt",
            "gestdown:fr"
        ]
    );
    assert_eq!(
        ranked[1].release,
        "Show.S01E02.720p.WEB-GrP (machine-translated)"
    );
    assert_eq!(ranked[1].source, "Subtitle Cat");
    assert_eq!(ranked[0].source, "TVsubtitles");
}
