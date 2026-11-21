use super::*;

fn rows(list: &[&str]) -> Vec<String> {
    list.iter().map(|row| row.to_string()).collect()
}

#[test]
fn a_row_names_its_height_by_number_before_by_word() {
    assert_eq!(
        row_height("Movie.2010.2160p.BluRay.REMUX — Service | 80 GB"),
        Some(2160)
    );
    assert_eq!(
        row_height("Movie.2010.Hybrid.1080p.UHD.BluRay.DV.HDR.x265"),
        Some(1080)
    );
    assert_eq!(row_height("Movie (2010) | 4K | HDR"), Some(2160));
    assert_eq!(row_height("Movie.720p.WEB.x264"), Some(720));
    assert_eq!(row_height("Movie.1080i.HDTV.MPEG2"), Some(1080));
    assert_eq!(row_height("Movie | SD | 700 MB"), Some(480));
    assert_eq!(row_height("Movie.2010.BluRay.x264"), None);
    assert_eq!(row_height("Movie.x264.h264"), None);
}

#[test]
fn a_1080p_play_never_takes_a_4k_copy_when_a_1080p_one_is_listed() {
    let list = rows(&[
        "Movie.2010.2160p.BluRay.REMUX.HEVC.DTS-HD — Service | 79 GB",
        "Movie.2010.2160p.WEB-DL.DV.HDR.H.265 — Service | 22 GB",
        "Movie.2010.1080p.BluRay.REMUX.VC-1.DTS-HD — Service | 30 GB",
        "Movie.2010.Hybrid.1080p.UHD.BluRay.DV.HDR.x265 — Service | 15 GB",
        "Movie.(2010).1080p.BluRay.x264.DTS-HD — Service | 18 GB",
        "Movie.2010.720p.BluRay.x264 — Service | 6 GB",
    ]);
    assert_eq!(pick_source(&list, 1080, &[], None), Some(4));
    // Tried and failed: the next 1080p copy, still not a 4K one.
    assert_eq!(pick_source(&list, 1080, &list[4..5], None), Some(2));
    assert_eq!(pick_source(&list, 720, &[], None), Some(5));
    assert_eq!(pick_source(&list, 2160, &[], None), Some(0));
}

#[test]
fn a_list_with_only_taller_copies_takes_the_smallest() {
    let list = rows(&[
        "Movie.2160p.REMUX",
        "Movie.1440p.WEB",
        "Movie.2160p.WEB.x264",
    ]);
    assert_eq!(pick_source(&list, 1080, &[], None), Some(1));
    let mixed = rows(&["Movie.2160p.WEB", "Movie.WEB.x264"]);
    assert_eq!(pick_source(&mixed, 1080, &[], None), Some(1));
}

#[test]
fn a_list_that_names_no_resolution_is_left_to_the_viewer() {
    assert_eq!(
        pick_source(&rows(&["Season 1", "Season 2"]), 1080, &[], None),
        None
    );
    let list = rows(&["Movie.1080p.x264"]);
    assert_eq!(pick_source(&list, 1080, &list, None), None);
    assert_eq!(pick_source(&[], 1080, &[], None), None);
}

fn labeled_pack() -> String {
    "Show.A.S03.1080P.WEB.H265-GRP — 1080P | SERVICE | 22.40 GB | CACHED PACK | PROVIDER | SEASON PACK | WEB | H.265"
            .to_string()
}

#[test]
fn a_season_pack_keeps_one_identity_across_episode_lists() {
    let labeled = labeled_pack();
    let other = "Show.A.S03.1080P.WEB.H265.GRP — 1080p | 22.40 GB | cached (pack) source | HEVC";
    let episode = "Show.A.S03E06.1080P.WEB.H265-GRP — 1080p | HEVC";
    let plain = "Show.B.S05.1080p.WEB.H264-GRP — 1080p | 18 GB | source";
    let id = pack_identity(&labeled).expect("pack");
    assert_eq!(pack_identity(other).as_deref(), Some(id.as_str()));
    assert_eq!(pack_identity(episode), None);
    assert_eq!(
        pack_identity("01. Show.A.S03.1080P.WEB.H265-GRP — 1080p | source"),
        Some(id.clone())
    );
    assert!(pack_identity(plain).is_some());
    assert_ne!(pack_identity(plain).as_deref(), Some(id.as_str()));
    assert_eq!(
        pack_identity("Show.Name.S02E01-E08.1080p.WEB-GRP — 1080p | 8 GB"),
        pack_identity("Show.Name.S02E01.E08.1080p.WEB-GRP — 1080p")
    );
    assert_eq!(
        pack_identity("Movie.2010.1080p.BluRay.x264 — Service | 18 GB"),
        None
    );
    assert_eq!(pack_identity("Show.3x06.1080p.WEB.x264 — 1080p"), None);
    assert_eq!(pack_identity("Hiking backpack — 1080p | x264"), None);
}

#[test]
fn a_remembered_pack_wins_over_a_lighter_file_at_the_same_height() {
    let pack = "Show.B.S05.1080p.WEB.H265-GRP — 1080p | 20 GB | source";
    let list = rows(&[
        pack,
        "Show.B.S05E12.1080p.WEB.x264-GRP — 1080p | x264",
        "Show.B.S05.2160p.WEB.H265-GRP — 2160p | HEVC",
    ]);
    let id = pack_identity(pack).expect("pack");
    // No saved pack yet: the season pack still beats the lighter episode.
    assert_eq!(pick_source(&list, 1080, &[], None), Some(0));
    assert_eq!(pick_source(&list, 1080, &[], Some(&id)), Some(0));
    assert_eq!(pick_source(&list, 1080, &list[0..1], Some(&id)), Some(1));
    let tall = pack_identity(&list[2]).expect("4k pack");
    assert_eq!(pick_source(&list, 1080, &[], Some(&tall)), Some(0));
    let shorter = "Show.B.S05.720p.WEB.H264-GRP — 720p | x264";
    let mixed = rows(&[
        "Show.B.S05E12.1080p.WEB.x264-GRP — 1080p | x264",
        shorter,
        pack,
    ]);
    // A fitting pack of any height beats a single. The closer pack wins.
    assert_eq!(pick_source(&mixed, 1080, &[], None), Some(2));
    assert_eq!(pick_source(&mixed, 1080, &mixed[2..3], None), Some(1));
    let saved_short = pack_identity(shorter).expect("720 pack");
    assert_eq!(pick_source(&mixed, 1080, &[], Some(&saved_short)), Some(1));
    assert_eq!(
        pick_source(&rows(&["Season 1", "Season PACK"]), 1080, &[], Some(&id)),
        None
    );
}

#[test]
fn a_remembered_pack_is_found_when_the_row_is_laid_out_differently() {
    let before = "Show.C.S02.1080p.BluRay.x265-GRP — 1080p | 18 GB | source";
    let id = pack_identity(before).expect("pack");
    // An add-on update that changes separators and adds words around the name.
    let after = "[CACHED] Show.C.S02.1080p.BluRay.x265-GRP / 18.0 GB / 1080p / source";
    let list = rows(&[
        "Show.C.S02.1080p.WEB.x264-OTHER — 1080p | 9 GB | source",
        "Show.C.S02E03.1080p.WEB.x264-OTHER — 1080p",
        after,
    ]);
    assert_eq!(pick_source(&list, 1080, &[], Some(&id)), Some(2));
    // An exact match still comes before a looser one.
    let exact = rows(&[after, before]);
    assert_eq!(pick_source(&exact, 1080, &[], Some(&id)), Some(1));
    // A name is matched as whole words, not inside a longer one.
    let longer = rows(&["Show.C.S02.1080p.BluRay.x265-GRPX — 1080p | source"]);
    assert_ne!(pack_rank(&longer[0], 1080, Some(&id)), 1);
}
