use super::*;

const SHOWS: &str = r##"<table><tr><td>#</td><td>TV Show</td><td>Seasons</td></tr>
<tr align="middle" bgcolor="#ffffff">
<td>1</td>
<td align=left style="padding: 0 4px;"><a href="tvshow-1-4.html"><b>Show One & Two</b></a></td>
<td>4</td>
<td>20</td>
<td>30</td>
<td>2021-2024</td>
</tr>
<tr align="middle" bgcolor="#ffffff">
<td>2</td>
<td align=left style="padding: 0 4px;"><a href="tvshow-2-15.html"><b>Some Show</b></a></td>
<td>15</td>
<td>40</td>
<td>90</td>
<td>2005-2020</td>
</tr>
<tr align="middle" bgcolor="#ffffff">
<td>3</td>
<td align=left style="padding: 0 4px;"><a href="tvshow-3-1.html"><b>Some Show: The Animation</b></a></td>
<td>1</td>
<td>10</td>
<td>25</td>
<td>2011-2011</td>
</tr>
<tr align="middle" bgcolor="#ffffff">
<td>4</td>
<td align=left style="padding: 0 4px;"><a href="tvshow-4-8.html"><b>Other Show</b></a></td>
<td>8</td>
<td>50</td>
<td>60</td>
<td>1998-2006</td>
</tr>
<tr align="middle" bgcolor="#ffffff">
<td>5</td>
<td align=left style="padding: 0 4px;"><a href="tvshow-5-4.html"><b>Other Show (2018)</b></a></td>
<td>4</td>
<td>25</td>
<td>26</td>
<td>2018-2022</td>
</tr>
<tr align="middle" bgcolor="#ffffff">
<td>6</td>
<td align=left style="padding: 0 4px;"><a href="tvshow-6-8.html"><b>The Third Show (US)</b></a></td>
<td>8</td>
<td>18</td>
<td>12</td>
<td>2010</td>
</tr>
<tr><td>9</td><td><a href="tvshow-x-1.html"><b>Broken</b></a></td></tr>
</table>"##;

const SEASON: &str = r##"<table id="table5">
<tr align="middle" bgcolor="#ffffff">
<td>1x03</td>
<td align=left style="padding: 0 4px;"><a href="episode-13.html"><b>Second Story</b></a></td>
<td>15</td>
<td><nobr><a href="subtitle-22.html"><img src="images/flags/en.gif" width="18" height="12" alt="en" border=0></a>&nbsp;</nobr></td>
</tr>
<tr align="middle" bgcolor="#ffffff">
<td>1x02</td>
<td align=left style="padding: 0 4px;"><a href="episode-12.html"><b>Pilot</b></a></td>
<td>15</td>
<td><nobr><a href="subtitle-21.html"><img src="images/flags/en.gif" width="18" height="12" alt="en" border=0></a>&nbsp;<a href="episode-12-pt.html"><img src="images/flags/pt.gif" width="18" height="12" alt="pt" border=0></a>&nbsp;</nobr></td>
</tr>
<tr align="middle" bgcolor="#ffffff">
<td>1x01</td>
<td align=left style="padding: 0 4px;"><a href="episode-11.html"><b>Unaired Pilot</b></a></td>
<td>14</td>
<td><nobr><a href="episode-11-en.html"><img src="images/flags/en.gif" width="18" height="12" alt="en" border=0></a>&nbsp;</nobr></td>
</tr>
</table>"##;

const EPISODE: &str = r##"<b>Subtitles for this episode:</b><div style="clear:both; padding-top:10px;"><span style="color:#666666"><b>English  subtitles</b></span></div>
<a href="/subtitle-21.html"><div title="Download English subtitles"  class="subtitlen" >
<div style="float:right; margin:0 2px;"><span style="color:black; font-weight:bold"><span style="color:red">0</span>/<span style="color:green">4</span></span></div>
    <h5 style="width:600px;"><img src="images/flags/en.gif" width="18" height="12" alt="" border=0 hspace=4 align=absmiddle>Some Show 1x02 </h5>
    <p style="width:110px; margin-left:50px" alt="rip" title="rip"><img src="images/rip.gif" width="16" height="16" alt="rip" title="rip" border=0 hspace=4 align="absmiddle">
	</p>
	<p style="width:100px;" alt="downloaded" title="downloaded"><img src="images/downloads.png" width="16" height="16" alt="downloaded" title="downloaded" border=0 hspace=4 align="absmiddle">
	120</p>
</div></a>
<div style="clear:both; padding-top:10px;"><span style="color:#666666"><b>Portuguese(br) subtitles</b></span></div>
<a href="/subtitle-23.html"><div title="Download Portuguese(br) subtitles"  class="subtitlen" >
    <h5 style="width:600px;"><img src="images/flags/br.gif" width="18" height="12" alt="" border=0 hspace=4 align=absmiddle>Some Show 1x02 (HDTV.GRP)</h5>
	<p style="width:100px;" alt="downloaded" title="downloaded"><img src="images/downloads.png" width="16" height="16" alt="downloaded" title="downloaded" border=0 hspace=4 align="absmiddle">
	30</p>
</div></a>
<a href="/subtitle-24.html"><div title="Download Greek subtitles"  class="subtitlen" >
    <h5 style="width:600px;"><img src="images/flags/gr.gif" width="18" height="12" alt="" border=0 hspace=4 align=absmiddle>Some Show 1x02 (HDTV.GRP)</h5>
</div></a>
<a href="/subtitle-1a.html"><div class="subtitlen"><h5><img src="images/flags/en.gif">Bad id</h5></div></a>
<a href="/subtitle-77.html"><div class="other"><img src="images/flags/en.gif"></div></a>
<a href="/subtitle-78.html"><div class="subtitlen"><img src="images/flags/xx.gif"><h5>Unknown flag</h5></div></a>"##;

const DOWNLOAD: &str = r##"<center><br /><div id="linkPlace"><b>Wait: <span id="timeNumer">0</span> sec ...</b></div></center>
<script type="text/javascript" language="JavaScript">
var timerFIG = 1;
        function startTimer()
{
        if(timerFIG !== 1) {
           timerFIG--;
        } else {
        	  var s1= 'fil';
        	  document.all.timeNumer.innerHTML = 'download starting';
           clearInterval(timer);
           var s2= 'es/S';
           var s3= 'ome_Sh';
           var s4= 'ow_1x02_en.zip';
           document.location = s1+s2+s3+s4;
        }
}
</script>"##;

#[test]
fn the_show_list_is_read_and_kept() {
    let shows = parse_shows(SHOWS);
    assert_eq!(shows.len(), 6);
    assert_eq!(
        shows[1],
        Show {
            id: 2,
            name: "Some Show".into(),
            first: Some(2005),
            last: Some(2020),
        }
    );
    assert_eq!(shows[0].name, "Show One & Two");
    assert_eq!((shows[5].first, shows[5].last), (Some(2010), None));
    assert_eq!(read_list(&write_list(&shows)), shows);
    assert!(read_list("x\t\t\tName\n1\t\t\t\n").is_empty());
}

#[test]
fn shows_are_matched_by_name_and_year() {
    let shows = parse_shows(SHOWS);
    let id = |name: &str, year: Option<u32>| pick_show(&shows, name, year).map(|show| show.id);
    assert_eq!(id("Some Show", None), Some(2));
    assert_eq!(id("some show", Some(2005)), Some(2));
    assert_eq!(id("Some Show Academy", None), None);
    assert_eq!(id("Show One and Two", None), Some(1));
    assert_eq!(id("Third Show (US)", None), Some(6));
    assert_eq!(id("The Third Show US", None), Some(6));
    // Of two shows sharing a name: the named year, the run, else the plain one.
    assert_eq!(id("Other Show", None), Some(4));
    assert_eq!(id("Other Show (2018)", None), Some(5));
    assert_eq!(id("Other Show", Some(2020)), Some(5));
    assert_eq!(id("Other Show", Some(2001)), Some(4));
    assert_eq!(id("Other Show", Some(1950)), Some(4));
    assert_eq!(id("", None), None);
}

#[test]
fn the_season_page_gives_the_episode() {
    assert_eq!(parse_season(SEASON, (1, 3), None), Some((13, (1, 3))));
    assert_eq!(
        parse_season(SEASON, (1, 3), Some("Second Story")),
        Some((13, (1, 3)))
    );
    // The site counts an unaired pilot as 1x01; the title finds the aired one.
    assert_eq!(
        parse_season(SEASON, (1, 1), Some("Pilot")),
        Some((12, (1, 2)))
    );
    assert_eq!(
        parse_season(SEASON, (1, 1), Some("Something else")),
        Some((11, (1, 1)))
    );
    assert_eq!(parse_season(SEASON, (1, 9), None), None);
    assert_eq!(parse_season("<html>Not found</html>", (1, 1), None), None);
}

#[test]
fn the_episode_page_lists_subtitles_in_wanted_languages() {
    let wanted = vec!["en".to_string(), "pt-PT".to_string()];
    let found = parse_episode(EPISODE, &wanted, (1, 2));
    let ids: Vec<&str> = found.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, vec!["tvsubtitles:21", "tvsubtitles:23"]);
    assert_eq!(found[0].release, "Some Show 1x02");
    assert_eq!(found[0].downloads, Some(120));
    assert_eq!(found[0].episode, Some((1, 2)));
    assert_eq!(found[1].language, "pt-BR");
    assert_eq!(found[1].release, "Some Show 1x02 (HDTV.GRP)");
    let greek = parse_episode(EPISODE, &["el".to_string()], (1, 2));
    assert_eq!(greek.len(), 1);
    assert_eq!(greek[0].downloads, None);
    assert!(parse_episode("<html>Just a moment...</html>", &wanted, (1, 2)).is_empty());
}

#[test]
fn the_download_page_spells_out_a_safe_zip_path() {
    assert_eq!(
        zip_path(DOWNLOAD).as_deref(),
        Some("files/Some_Show_1x02_en.zip")
    );
    let spaced = "var a= 'files/Show Name'; var b= ' (2005) 1x01.zip'; document.location = a+b;";
    assert_eq!(
        zip_path(spaced).as_deref(),
        Some("files/Show Name (2005) 1x01.zip")
    );
    for bad in [
        "var a= 'files/../x.zip'; document.location = a;",
        "var a= 'https://evil/x.zip'; document.location = a;",
        "var a= 'files/a/b.zip'; document.location = a;",
        "var a= 'files/x.exe'; document.location = a;",
        "var a= 'files/x.zip'; document.location = a+missing;",
        "var a= 'files/x.zip'; document.location = a+'y';",
        "<html>Just a moment...</html>",
    ] {
        assert_eq!(zip_path(bad), None, "{bad}");
    }
    assert!(valid_id("21"));
    assert!(!valid_id("14a"));
    assert!(!valid_id(""));
}

#[test]
fn flags_name_languages() {
    assert_eq!(flag_language("br"), Some("pt-BR"));
    assert_eq!(flag_language("pt"), Some("pt-PT"));
    assert_eq!(flag_language("gr"), Some("el"));
    assert_eq!(flag_language("en"), Some("en"));
    assert_eq!(flag_language("blank"), None);
}

#[tokio::test]
async fn a_fresh_kept_list_is_used_without_asking_the_site() {
    let dir = std::env::temp_dir().join(format!("portal-tvsubtitles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(LIST_FILE), "2\t2005\t2020\tSome Show\n").unwrap();
    let list = ShowList::open(Some(&dir));
    // A client that cannot connect: the kept list must be enough.
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
        .build()
        .unwrap();
    let shows = list.get(&client).await.unwrap();
    assert_eq!(shows[0].id, 2);
    let _ = std::fs::remove_dir_all(&dir);
}
