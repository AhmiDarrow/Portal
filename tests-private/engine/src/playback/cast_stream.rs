use super::*;

#[test]
fn a_title_is_cut_into_pieces_on_a_fixed_grid() {
    assert_eq!(piece_count(888.0), 296);
    assert_eq!(piece_count(888.3), 296);
    assert_eq!(piece_count(889.0), 297);
    assert_eq!(piece_count(2.0), 1);
    assert_eq!(piece_at(0.0, 296), 0);
    assert_eq!(piece_at(2.9, 296), 0);
    assert_eq!(piece_at(3.0, 296), 1);
    assert_eq!(piece_at(700.5, 296), 233);
    assert_eq!(piece_at(99_999.0, 296), 295);
    assert_eq!(piece_at(f64::NAN, 296), 0);
}

#[test]
fn the_playlist_lists_the_whole_title() {
    let text = cast_playlist(10.0);
    assert!(text.starts_with("#EXTM3U\n"));
    assert!(text.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
    assert!(text.contains("#EXT-X-MAP:URI=\"castinit.mp4\"\n"));
    assert!(text.ends_with("#EXTINF:1.000000,\nc00003.m4s\n#EXT-X-ENDLIST\n"));
    assert_eq!(text.matches("#EXTINF:3.000000,").count(), 3);
    assert_eq!(text.matches(".m4s").count(), 4);
}

#[test]
fn only_cast_names_are_cast_files() {
    assert!(cast_name("cast.m3u8"));
    assert!(cast_name("castinit.mp4"));
    assert!(cast_name("c00042.m4s"));
    assert_eq!(piece_index("c00042.m4s"), Some(42));
    assert!(!cast_name("c42.m4s"));
    assert!(!cast_name("r0-00042.m4s"));
    assert!(!cast_name("init0.mp4"));
    assert!(!cast_name("../c00042.m4s"));
    assert!(valid_media_name("c00042.m4s"));
    assert!(!valid_media_name("run0.m3u8"));
}

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn init_file() -> Vec<u8> {
    let mut tkhd = vec![0u8; 12];
    tkhd.extend_from_slice(&7u32.to_be_bytes());
    let mut mdhd = vec![0u8; 12];
    mdhd.extend_from_slice(&48_000u32.to_be_bytes());
    let mdia = atom(b"mdia", &atom(b"mdhd", &mdhd));
    let edts = atom(b"edts", &atom(b"elst", &[0u8; 16]));
    let trak = atom(b"trak", &[atom(b"tkhd", &tkhd), edts, mdia].concat());
    [atom(b"ftyp", b"iso5"), atom(b"moov", &trak)].concat()
}

fn piece_file(base: u64) -> Vec<u8> {
    let mut tfhd = vec![0u8; 4];
    tfhd.extend_from_slice(&7u32.to_be_bytes());
    let mut tfdt = vec![1u8, 0, 0, 0];
    tfdt.extend_from_slice(&base.to_be_bytes());
    let traf = atom(
        b"traf",
        &[atom(b"tfhd", &tfhd), atom(b"tfdt", &tfdt)].concat(),
    );
    [
        atom(b"styp", b"msdh"),
        atom(b"sidx", &[0u8; 20]),
        atom(b"moof", &traf),
        atom(b"mdat", b"picture"),
    ]
    .concat()
}

#[test]
fn a_piece_is_moved_to_the_titles_clock() {
    let init = init_file();
    let scales = track_scales(&init);
    assert_eq!(scales, vec![(7, 48_000)]);
    let moved = shift_piece(piece_file(144_000), &scales, 300);
    assert_eq!(moved.len(), piece_file(0).len());
    assert_eq!(moved, {
        let mut want = piece_file(144_000 + 300 * 48_000);
        let at = want.windows(4).position(|kind| kind == b"sidx").unwrap();
        want[at..at + 4].copy_from_slice(b"free");
        want
    });
    // A piece of the run from the start keeps its times and loses its index.
    let kept = shift_piece(piece_file(144_000), &scales, 0);
    assert!(!kept.windows(4).any(|kind| kind == b"sidx"));
    assert!(kept.ends_with(b"picture"));
}

#[test]
fn the_start_file_loses_its_edit_lists() {
    let plain = plain_init(init_file());
    assert_eq!(plain.len(), init_file().len());
    assert!(!plain.windows(4).any(|kind| kind == b"edts"));
    assert_eq!(track_scales(&plain), vec![(7, 48_000)]);
}

#[test]
fn broken_boxes_are_left_alone() {
    assert!(track_scales(b"not an mp4 file at all").is_empty());
    let torn = vec![0, 0, 0, 200, b'm', b'o', b'o', b'f', 1, 2, 3];
    assert_eq!(shift_piece(torn.clone(), &[(1, 1000)], 9), torn);
    assert_eq!(plain_init(Vec::new()), Vec::<u8>::new());
}
