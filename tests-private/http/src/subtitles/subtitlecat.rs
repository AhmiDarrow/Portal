use super::*;

const LISTING: &str = r#"<table class="table sub-table"><thead><tr><th colspan="2"><h2 class="sec-title">6 subtitles found</h2></th></tr></thead><tbody>
	<tr>
		<td><a href="subs/1/Showname%20S01E01%20-%20First%20Night.html">Showname S01E01 - First Night</a> (translated from English)</td>
		<td class="sub-table__metric"><span class="sub-table__metric-label"><img src="assets/images/arrow-down.png" alt=""> Downloads</span><span class="sub-table__metric-value">1<span class="sub-table__metric-unit"> downloads</span></span></td>
	</tr>
	<tr>
		<td><a href="subs/2/Showname%20Academy%20S01E01%20Second%20Chance.html">Showname Academy S01E01 Second Chance</a> (translated from Spanish)</td>
		<td class="sub-table__metric"><span class="sub-table__metric-label"><img src="assets/images/arrow-down.png" alt=""> Downloads</span><span class="sub-table__metric-value">3<span class="sub-table__metric-unit"> downloads</span></span></td>
	</tr>
	<tr>
		<td><a href="subs/3/Showname.S01E01.Pilot.1080p.WEB-DL.html">Showname.S01E01.Pilot.1080p.WEB-DL</a> (translated from Spanish)</td>
		<td class="sub-table__metric"><span class="sub-table__metric-label"><img src="assets/images/arrow-down.png" alt=""> Downloads</span><span class="sub-table__metric-value">5<span class="sub-table__metric-unit"> downloads</span></span></td>
	</tr>
	<tr>
		<td><a href="subs/4/Showname.S01E01.WEB-GRP.html">Showname.S01E01.WEB-GRP</a> (translated from English)</td>
		<td class="sub-table__metric"><span class="sub-table__metric-label"><img src="assets/images/arrow-down.png" alt=""> Downloads</span><span class="sub-table__metric-value">50<span class="sub-table__metric-unit"> downloads</span></span></td>
	</tr>
	<tr>
		<td><a href="subs/5/Showname.S01E02.Second.Story.html">Showname.S01E02.Second.Story</a> (translated from English)</td>
	</tr>
	<tr>
		<td><a href="subs/6/..%2F..%2Fevil.html">evil</a></td>
	</tr>
</tbody></table>"#;

const PAGE: &str = r#"<div class="all-sub"><h2 class="sec-title">All language subtitles for Showname.S01E01.WEB-GRP</h2>
<div class="sub-single"><span><img src="/assets/flags/za.png" alt="af" class="flag"></span><span>Afrikaans</span> <span><button id="af" onclick="translate_from_server_folder('af', 'Showname.S01E01.WEB-GRP-orig.srt', '/subs/4/')" class="yellow-link">Translate</button></span></div>
<div class="sub-single"><span><img src="/assets/flags/al.png" alt="sq" class="flag"></span><span>Albanian</span>
<span><a id="download_sq"  onclick="log_download(1); show_voting('sq');" href="/subs/7/Showname.S01E01.WEB-GRP-sq.srt" class="green-link">Download</a></span></div>
<div class="sub-single"><span><img src="/assets/flags/gb.png" alt="en" class="flag"></span><span>English</span>
<span><a id="download_en"  onclick="log_download(2); show_voting('en');" href="/subs/8/Showname.S01E01.WEB-GRP-en.srt" class="green-link">Download</a>    <span id="voting_en" style="display:none;"><a href="javascript:vote('en',2,+1)">+</a></span></span></div>
<div class="sub-single"><span>Spanish</span><span><a id="download_es" href="/subs/9/Showname.S01E01.WEB-GRP-es.srt" class="green-link">Download</a></span></div>
<div class="sub-single"><span>Spanish (Latin America)</span><span><a id="download_es-419" href="/subs/10/Showname.S01E01.WEB-GRP-es-419.srt" class="green-link">Download</a></span></div>
<div class="sub-single"><span>Portuguese (Brazil)</span><span><a id="download_pt-BR" href="/subs/11/Showname.S01E01.WEB-GRP-pt-BR.srt" class="green-link">Download</a></span></div>
<div class="sub-single"><span>Hebrew</span><span><a id="download_iw" href="/subs/12/Showname%20S01E01-iw.srt" class="green-link">Download</a></span></div>
<div class="sub-single"><span>Kurdish</span><span><a id="download_ku" href="/subs/13/Showname.S01E01.WEB-GRP-ku.srt" class="green-link">Download</a></span></div>
<div class="sub-single"><span>French</span><span><a id="download_fr" href="https://evil.example/x.srt" class="green-link">Download</a></span></div>
</div>"#;

fn episode_search() -> Search {
    Search {
        title: "Showname".into(),
        year: None,
        episode: Some((1, 1)),
    }
}

#[test]
fn the_listing_is_read() {
    let rows = parse_listing(LISTING);
    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows[0].path,
        "subs/1/Showname S01E01 - First Night.html"
    );
    assert_eq!(rows[0].name, "Showname S01E01 - First Night");
    assert_eq!(rows[0].original, Some("en"));
    assert_eq!(rows[0].downloads, Some(1));
    assert_eq!(rows[2].original, Some("es"));
    assert_eq!(rows[4].downloads, None);
    assert!(parse_listing("<html>Just a moment...</html>").is_empty());
}

#[test]
fn only_rows_for_the_asked_episode_are_read() {
    let wanted = vec!["en".to_string()];
    let rows = best_rows(parse_listing(LISTING), &episode_search(), &wanted, 3);
    let paths: Vec<&str> = rows.iter().map(|row| row.path.as_str()).collect();
    // Made in English first, most downloaded first; no "Academy", no S01E02.
    assert_eq!(
        paths,
        vec![
            "subs/4/Showname.S01E01.WEB-GRP.html",
            "subs/1/Showname S01E01 - First Night.html",
            "subs/3/Showname.S01E01.Pilot.1080p.WEB-DL.html",
        ]
    );
    assert_eq!(query_text(&episode_search()), "Showname S01E01");
}

#[test]
fn names_are_matched_to_episodes_and_movies() {
    let episode = episode_search();
    assert!(names_it("Showname - s01 e01 REMUX.eng", &episode));
    assert!(names_it("The Showname 1x01", &episode));
    assert!(names_it("Showname (2005) S01E01", &episode));
    assert!(!names_it("Showname Academy S01E01", &episode));
    assert!(!names_it("Showname S01E10", &episode));
    assert!(!names_it("Showname", &episode));
    let movie = Search {
        title: "Storm".into(),
        year: Some(1995),
        episode: None,
    };
    assert_eq!(query_text(&movie), "Storm 1995");
    assert!(names_it("Storm.1995.1080p.BluRay.x264-GRP", &movie));
    assert!(names_it("Storm (1996)", &movie));
    assert!(!names_it("Storm.2013.720p", &movie));
    assert!(!names_it("Storm Front 1995", &movie));
    assert!(!names_it("Storm 1080p", &movie));
    let numbered = Search {
        title: "1234".into(),
        year: Some(2019),
        episode: None,
    };
    assert!(names_it("1234.2019.1080p.WEB", &numbered));
    let no_year = Search {
        title: "Storm".into(),
        year: None,
        episode: None,
    };
    assert!(names_it("Storm 1080p BluRay", &no_year));
    assert!(names_it("Storm", &no_year));
    assert!(!names_it("Storm Front 1080p", &no_year));
}

#[test]
fn page_files_are_originals_or_marked_translations() {
    let row = Row {
        path: "subs/4/Showname.S01E01.WEB-GRP.html".into(),
        name: "Showname.S01E01.WEB-GRP".into(),
        original: Some("en"),
        downloads: Some(50),
    };
    let wanted: Vec<String> = ["en", "es", "pt-PT", "he"].map(String::from).to_vec();
    let found = parse_page(PAGE, &row, &wanted, Some((1, 1)));
    let ids: Vec<&str> = found.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "subtitlecat:subs/8/Showname.S01E01.WEB-GRP-en.srt",
            "subtitlecat:subs/9/Showname.S01E01.WEB-GRP-es.srt",
            "subtitlecat:subs/11/Showname.S01E01.WEB-GRP-pt-BR.srt",
            "subtitlecat:subs/12/Showname S01E01-iw.srt",
        ]
    );
    assert!(!found[0].machine_translated);
    assert!(found[1..].iter().all(|item| item.machine_translated));
    assert_eq!(found[2].language, "pt-BR");
    assert_eq!(found[3].language, "he");
    assert_eq!(found[0].episode, Some((1, 1)));
    assert_eq!(found[0].release, "Showname.S01E01.WEB-GRP");

    // Made in Spanish: es-419 is a translation, es the original.
    let spanish = Row {
        original: Some("es"),
        ..row.clone()
    };
    let found = parse_page(PAGE, &spanish, &["es".to_string()], None);
    assert_eq!(found.len(), 1);
    assert!(!found[0].machine_translated);
    // An unknown original makes every file a translation.
    let unknown = Row {
        original: None,
        ..row
    };
    let found = parse_page(PAGE, &unknown, &["en".to_string()], None);
    assert!(found[0].machine_translated);
    assert!(parse_page("<html>changed</html>", &spanish, &wanted, None).is_empty());
}

#[test]
fn paths_are_checked() {
    assert!(valid_id("subs/124/Show.S01E01-en.srt"));
    assert!(valid_id("subs/932/Show S01E01 (1)-iw.srt"));
    for bad in [
        "subs/124/../x.srt",
        "subs/124/a/b.srt",
        "subs/12a/x.srt",
        "subs/124/x.html",
        "subs/124/.srt",
        "/subs/124/x.srt",
        "other/124/x.srt",
        "subs/124/x?.srt",
        "subs/124/x\\y.srt",
        "",
    ] {
        assert!(!valid_id(bad), "{bad}");
    }
    assert_eq!(
        site_path("/subs/1/A%20B-en.srt", ".srt").as_deref(),
        Some("subs/1/A B-en.srt")
    );
    assert_eq!(site_path("/subs/1/%2E%2E%2Fx.srt", ".srt"), None);
    assert_eq!(site_path("/subs/1/bad%zz.srt", ".srt"), None);
    assert_eq!(site_path("/subs/1/bad%ff.srt", ".srt"), None);
    assert_eq!(
        site_url("subs/1/A B#-en.srt")
            .map(|url| url.to_string())
            .ok(),
        Some("https://www.subtitlecat.com/subs/1/A%20B%23-en.srt".to_string())
    );
}
