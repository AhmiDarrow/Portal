use super::*;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

fn sample_index() -> &'static str {
    r#"<addons>
          <addon id="plugin.video.sample" name="Sample" version="1.2.3">
            <summary lang="fr">Bonjour</summary>
            <summary lang="en">A sample video add-on.</summary>
            <requires>
              <import addon="xbmc.python" version="3.0.0"/>
              <import addon="script.module.need" version="1.0.0"/>
              <import addon="script.module.maybe" optional="true"/>
              <import addon="script.module.missing" version="1.0.0"/>
            </requires>
            <extension point="xbmc.python.pluginsource" library="default.py">
              <provides>video</provides>
            </extension>
          </addon>
          <addon id="script.module.need" name="Need" version="1.0.0">
            <extension point="xbmc.python.module" library="lib"/>
          </addon>
          <addon id="plugin.video.gone" name="Gone" version="1.0.0">
            <broken>retired</broken>
            <extension point="xbmc.python.pluginsource"><provides>video</provides></extension>
          </addon>
          <addon id="plugin.video.music" name="Music" version="1.0.0">
            <extension point="xbmc.python.pluginsource"><provides>audio</provides></extension>
          </addon>
          <addon id="plugin.video.legacy" name="Legacy" version="0.1.0">
            <extension point="xbmc.addon.video" library="default.py"/>
          </addon>
        </addons>"#
}

fn video_xml(id: &str) -> String {
    format!(
        r#"<addon id="{id}" name="Sample" version="1.2.3">
                <extension point="xbmc.python.pluginsource" library="default.py">
                  <provides>video</provides>
                </extension>
              </addon>"#
    )
}

fn repo_xml() -> &'static str {
    r#"<addon id="repository.sample" name="Sample Repo" version="1.0.0">
            <extension point="xbmc.addon.repository" name="Sample Repo">
              <dir target="linux">
                <info compressed="true">https://linux.example/addons.xml</info>
                <datadir zip="true">https://linux.example/</datadir>
              </dir>
              <dir>
                <info compressed="true">https://all.example/addons.xml</info>
                <checksum>https://all.example/addons.xml.md5</checksum>
                <datadir zip="true">https://all.example/zips</datadir>
              </dir>
            </extension>
          </addon>"#
}

fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn package_url_joins_the_datadir() {
    assert_eq!(
        package_url("https://example.test/repo", "plugin.video.sample", "1.2.3").as_deref(),
        Some("https://example.test/repo/plugin.video.sample/plugin.video.sample-1.2.3.zip")
    );
    assert_eq!(
        package_url("https://example.test/repo/", "plugin.video.sample", "1.2.3").as_deref(),
        Some("https://example.test/repo/plugin.video.sample/plugin.video.sample-1.2.3.zip")
    );
    assert!(package_url("https://example.test/repo/", "plugin.video.sample", "1 2").is_none());
    assert!(package_url("http://example.test/repo/", "plugin.video.sample", "1").is_none());
    assert!(package_url(
        "https://user:pass@example.test/repo/",
        "plugin.video.a",
        "1"
    )
    .is_none());
}

#[test]
fn zip_address_rejects_userinfo_and_plain_http() {
    assert!(require_zip_address("https://example.test/a.zip").is_ok());
    assert!(require_zip_address("https://example.test/a.zip?x=1").is_ok());
    assert!(require_zip_address("http://example.test/a.zip").is_err());
    assert!(require_zip_address("https://example.test/a.txt").is_err());
    assert!(require_zip_address("https://user@example.test/a.zip").is_err());
    assert!(require_zip_address("https://example.test/a.zip\\..").is_err());
    assert!(require_zip_address("").is_err());
}

#[test]
fn repo_root_html_picks_repository_zip() {
    let html = r#"<!DOCTYPE html>
<a href="plugin.video.example-1.0.0.zip">plugin</a>
<a href="repository.example-2.2.6.zip">repository.example-2.2.6.zip</a>
"#;
    let zip = pick_repo_zip_from_html(html, "https://repo.example.test").unwrap();
    assert_eq!(
        zip,
        "https://repo.example.test/repository.example-2.2.6.zip"
    );
    // A folder with no trailing slash still contains the zip. Joining it
    // to the parent is how https://example.github.io/packages 404'd.
    assert_eq!(
        pick_repo_zip_from_html(
            r#"<a href="repository.packages-2.2.0.zip">z</a>"#,
            "https://example.github.io/packages",
        )
        .unwrap(),
        "https://example.github.io/packages/repository.packages-2.2.0.zip"
    );
    assert_eq!(
        pick_repo_zip_from_html(
            r#"<a href="repository.example-1.0.zip">z</a>"#,
            "https://repo.example.test/index.html",
        )
        .unwrap(),
        "https://repo.example.test/repository.example-1.0.zip"
    );
    assert_eq!(
        pick_repo_zip_from_html(
            r#"<a href="repository.example-1.0.zip">z</a>"#,
            "https://repo.example.test/packages?ref=1",
        )
        .unwrap(),
        "https://repo.example.test/packages/repository.example-1.0.zip"
    );
    let abs = pick_repo_zip_from_html(
        r#"<a href="https://repo.example.test/repository.example-2.2.6.zip">r</a>"#,
        "https://repo.example.test/",
    )
    .unwrap();
    assert!(abs.ends_with("repository.example-2.2.6.zip"));
    assert!(pick_repo_zip_from_html(
        r#"<a href="plugin.video.only-1.0.0.zip">x</a>"#,
        "https://example.test/"
    )
    .is_err());
    assert!(pick_repo_zip_from_html("<p>none</p>", "https://example.test/").is_err());
}

#[test]
fn an_unquoted_link_ending_in_a_wide_space_is_read() {
    let html = "<a href=repository.example-1.0.zip\u{a0}title=x>r</a><A HREF=\"b\">b</A>";
    assert_eq!(
        html_hrefs(html),
        vec!["repository.example-1.0.zip".to_string(), "b".to_string()]
    );
    assert_eq!(
        pick_repo_zip_from_html(html, "https://repo.example.test/").unwrap(),
        "https://repo.example.test/repository.example-1.0.zip"
    );
    assert!(html_hrefs("<a href= >\u{e9}</a>").is_empty());
}

#[test]
fn overlay_name_keeps_a_short_label() {
    assert_eq!(overlay_name(None), None);
    assert_eq!(overlay_name(Some("   ")), None);
    assert_eq!(
        overlay_name(Some("  Example  ")).as_deref(),
        Some("Example")
    );
    assert_eq!(overlay_name(Some("a\nb")), None);
    assert_eq!(overlay_name(Some(&"n".repeat(81))), None);
    assert_eq!(
        overlay_name(Some(&"n".repeat(80))).as_deref(),
        Some("n".repeat(80).as_str())
    );
}

#[test]
fn index_keeps_video_modules_and_skips_broken() {
    let packages = parse_index(sample_index(), "https://example.test/repo/").unwrap();
    let ids: Vec<&str> = packages.iter().map(|package| package.id.as_str()).collect();
    assert!(ids.contains(&"plugin.video.sample"));
    assert!(ids.contains(&"script.module.need"));
    assert!(ids.contains(&"plugin.video.legacy"));
    assert!(!ids.contains(&"plugin.video.gone"));
    assert!(!ids.contains(&"plugin.video.music"));
    let sample = packages
        .iter()
        .find(|package| package.id == "plugin.video.sample")
        .unwrap();
    assert!(sample.video);
    assert_eq!(sample.summary, "A sample video add-on.");
    assert!(sample
        .imports
        .iter()
        .any(|import| import.addon == "script.module.need" && !import.optional));
    assert!(sample
        .imports
        .iter()
        .all(|import| import.addon != "xbmc.python"));
    assert!(!packages
        .iter()
        .any(|package| package.id == "script.module.need" && package.video));
    assert!(packages
        .iter()
        .any(|package| package.id == "plugin.video.legacy" && package.video));
}

#[test]
fn a_stray_byte_in_an_index_keeps_the_repository_readable() {
    let mut bytes = sample_index().as_bytes().to_vec();
    let at = sample_index().find("Bonjour").unwrap();
    bytes[at] = 0xE9;
    let text = utf8_xml(&bytes);
    let packages = parse_index(&text, "https://example.test/repo/").unwrap();
    assert!(packages
        .iter()
        .any(|package| package.video && package.id == "plugin.video.sample"));
}

#[test]
fn deps_install_modules_and_skip_optional() {
    let packages = parse_index(
        r#"<addons>
              <addon id="plugin.video.sample" name="Sample" version="1.0.0">
                <requires>
                  <import addon="xbmc.python" version="3.0.0"/>
                  <import addon="script.module.need" version="1.0.0"/>
                  <import addon="script.module.maybe" optional="true"/>
                </requires>
                <extension point="xbmc.python.pluginsource"><provides>video</provides></extension>
              </addon>
              <addon id="script.module.need" name="Need" version="1.0.0">
                <extension point="xbmc.python.module" library="lib"/>
              </addon>
            </addons>"#,
        "https://example.test/repo/",
    )
    .unwrap();
    let order = plan_deps(&packages, "plugin.video.sample", &|_| false).unwrap();
    assert_eq!(
        order,
        vec![
            "script.module.need".to_string(),
            "plugin.video.sample".to_string()
        ]
    );
    let installed = plan_deps(&packages, "plugin.video.sample", &|id| {
        id == "script.module.need"
    })
    .unwrap();
    assert_eq!(installed, vec!["plugin.video.sample".to_string()]);
    let missing = parse_index(sample_index(), "https://example.test/repo/").unwrap();
    assert!(plan_deps(&missing, "plugin.video.sample", &|_| false).is_err());
}

#[test]
fn feed_prefers_an_open_target() {
    let feed = read_feed(repo_xml()).unwrap();
    assert_eq!(feed.info, "https://all.example/addons.xml");
    assert!(feed.compressed);
    assert_eq!(feed.datadir, "https://all.example/zips/");
    let windows = read_feed(
            r#"<addon id="repository.sample" name="Sample" version="1">
                <extension point="xbmc.addon.repository">
                  <dir target="linux"><info compressed="false">https://linux.example/addons.xml</info><datadir zip="true">https://linux.example/</datadir></dir>
                  <dir target="windows"><info compressed="false">https://win.example/addons.xml</info><datadir zip="true">https://win.example/</datadir></dir>
                </extension>
              </addon>"#,
        )
        .unwrap();
    assert_eq!(windows.info, "https://win.example/addons.xml");
}

#[test]
fn kodi_21_keeps_every_matching_repository_directory() {
    let xml = r#"<addon id="repository.packages" name="Packages Repo" version="2.2.0">
            <extension point="xbmc.addon.repository">
              <dir minversion="18.0">
                <info compressed="false">https://example.test/repo/addons.xml</info>
                <datadir zip="true">https://example.test/repo/zips/</datadir>
              </dir>
              <dir minversion="18.0" maxversion="18.9.0">
                <info compressed="false">https://example.test/leia/addons.xml</info>
                <datadir zip="true">https://example.test/leia/zips/</datadir>
              </dir>
              <dir minversion="18.9.0">
                <info compressed="false">https://example.test/matrix/addons.xml</info>
                <datadir zip="true">https://example.test/matrix/zips/</datadir>
              </dir>
            </extension>
          </addon>"#;
    let feeds = read_feeds(xml, Some("21.3"), true).unwrap();
    let infos: Vec<&str> = feeds.iter().map(|feed| feed.info.as_str()).collect();
    assert_eq!(
        infos,
        vec![
            "https://example.test/repo/addons.xml",
            "https://example.test/matrix/addons.xml",
        ]
    );
    let leia = read_feeds(xml, Some("18.5"), true).unwrap();
    assert!(leia.iter().any(|feed| feed.info.contains("/leia/")));
    assert!(leia.iter().all(|feed| !feed.info.contains("/matrix/")));
}

#[test]
fn an_official_repository_without_zip_true_is_used_for_dependencies() {
    let xml = r#"<addon id="repository.xbmc.org" name="Kodi" version="3.4.0">
            <extension point="xbmc.addon.repository">
              <dir>
                <info>https://mirrors.kodi.tv/addons/omega/addons.xml.gz</info>
                <datadir>https://mirrors.kodi.tv/addons/omega</datadir>
              </dir>
            </extension>
          </addon>"#;
    assert!(read_feeds(xml, Some("21.3"), true).is_err());
    let feeds = read_feeds(xml, Some("21.3"), false).unwrap();
    assert_eq!(feeds[0].datadir, "https://mirrors.kodi.tv/addons/omega/");
}

#[test]
fn a_higher_version_replaces_the_same_add_on() {
    let older = parse_index(
        r#"<addons>
              <addon id="plugin.video.beta" name="Beta" version="2.1.9">
                <extension point="xbmc.python.pluginsource"><provides>video</provides></extension>
              </addon>
            </addons>"#,
        "https://example.test/repo/",
    )
    .unwrap();
    let newer = parse_index(
        r#"<addons>
              <addon id="plugin.video.beta" name="Beta" version="3.0.1">
                <extension point="xbmc.python.pluginsource"><provides>video</provides></extension>
              </addon>
              <addon id="script.module.textlib" name="Textlib" version="1.3.6">
                <extension point="xbmc.python.module" library="lib"/>
              </addon>
            </addons>"#,
        "https://example.test/matrix/",
    )
    .unwrap();
    let mut packages = older;
    merge_packages(&mut packages, newer);
    let beta = packages
        .iter()
        .find(|package| package.id == "plugin.video.beta")
        .unwrap();
    assert_eq!(beta.version, "3.0.1");
    assert!(packages
        .iter()
        .any(|package| package.id == "script.module.textlib"));
}

#[test]
fn a_second_provides_does_not_hide_video() {
    let packages = parse_index(
        r#"<addons>
              <addon id="plugin.video.tube" name="Tube" version="7.4.4+unofficial.2">
                <extension point="xbmc.python.pluginsource">
                  <provides>video</provides>
                </extension>
                <extension point="xbmc.python.script">
                  <provides>executable</provides>
                </extension>
              </addon>
            </addons>"#,
        "https://example.test/repo/",
    )
    .unwrap();
    let tube = packages
        .iter()
        .find(|package| package.id == "plugin.video.tube")
        .unwrap();
    assert!(tube.video);
}

#[test]
fn repository_client_names_itself_kodi() {
    let portal = concat!("Portal/", env!("CARGO_PKG_VERSION"));
    assert_eq!(
        repository_user_agent(Some("21.3")),
        format!("{portal} Kodi/21.3")
    );
    assert_eq!(repository_user_agent(Some("  ")), format!("{portal} Kodi"));
    assert_eq!(repository_user_agent(None), format!("{portal} Kodi"));
}

#[test]
fn an_html_page_is_not_an_addon_index() {
    assert!(!looks_like_xml(
        b"<!DOCTYPE html><html><body><addons></addons></body></html>"
    ));
    assert!(!looks_like_xml(b"<html><body>Search</body></html>"));
    assert!(!looks_like_xml(b"d41d8cd98f00b204e9800998ecf8427e\n"));
    assert!(looks_like_xml(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<addons></addons>"#
    ));
    assert!(looks_like_xml(b"\n<!-- note --><addons/>"));
    assert!(looks_like_xml(&[
        0xEF, 0xBB, 0xBF, b'<', b'a', b'd', b'd', b'o', b'n', b's', b'>'
    ]));
}

#[test]
fn one_unreadable_directory_does_not_drop_the_others() {
    let good = parse_index(sample_index(), "https://example.test/repo/");
    let bad = Err(EngineError::Message(
        "That repository could not be read.".into(),
    ));
    let merged = merge_feed_results(vec![bad, good]).unwrap();
    assert!(merged
        .iter()
        .any(|package| package.id == "plugin.video.sample"));
    let none = merge_feed_results(vec![
        Err(EngineError::Message(
            "That repository did not answer.".into(),
        )),
        Err(EngineError::Message(
            "That repository could not be read.".into(),
        )),
    ]);
    let Err(err) = none else {
        panic!("every directory failed");
    };
    assert_eq!(err.public_message(), "That repository could not be read.");
}

#[test]
fn file_version_reads_the_marked_string() {
    let mut bytes = Vec::new();
    for unit in "FileVersion".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    for unit in "21.3".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    assert_eq!(version_marked(&bytes).as_deref(), Some("21.3"));
    assert!(version_fits("21.3", "18.9.0", ""));
    assert!(!version_fits("21.3", "18.0", "18.9.0"));
    assert!(version_fits("18.5", "18.0", "18.9.0"));
}

#[test]
fn gzip_index_roundtrip() {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(sample_index().as_bytes()).unwrap();
    let bytes = encoder.finish().unwrap();
    let decoded = inflate_if_gzip(&bytes).unwrap();
    let packages = parse_index(
        &String::from_utf8(decoded).unwrap(),
        "https://example.test/repo/",
    )
    .unwrap();
    assert!(packages
        .iter()
        .any(|package| package.video && package.id == "plugin.video.sample"));
}

#[test]
fn extract_rejects_parent_and_absolute_paths() {
    let xml = video_xml("plugin.video.sample");
    let bytes = zip_bytes(&[
        ("plugin.video.sample/addon.xml", xml.as_bytes()),
        ("../outside.txt", b"nope"),
        ("/tmp/evil.txt", b"nope"),
    ]);
    let staged = stage_zip(&bytes, Accept::UserChoice).unwrap();
    assert_eq!(staged.id, "plugin.video.sample");
    assert!(staged
        .files
        .iter()
        .all(|(rel, _)| rel != "outside.txt" && rel != "evil.txt"));
    assert!(staged.files.iter().any(|(rel, _)| rel == "addon.xml"));
}

#[test]
fn two_addons_and_a_skin_are_refused() {
    let one = video_xml("plugin.video.one");
    let two = video_xml("plugin.video.two");
    let both = zip_bytes(&[
        ("plugin.video.one/addon.xml", one.as_bytes()),
        ("plugin.video.two/addon.xml", two.as_bytes()),
    ]);
    assert!(stage_zip(&both, Accept::UserChoice).is_err());
    let skin = r#"<addon id="skin.sample" name="Skin" version="1"><extension point="xbmc.gui.skin" /></addon>"#;
    let packed = zip_bytes(&[("skin.sample/addon.xml", skin.as_bytes())]);
    let err = match stage_zip(&packed, Accept::UserChoice) {
        Err(err) => err,
        Ok(_) => panic!("a skin zip was accepted"),
    };
    assert_eq!(
        err.public_message(),
        "Portal installs a repository, a video add-on, or a module video add-ons use."
    );
}

#[test]
fn an_entry_that_understates_its_size_is_still_capped() {
    let xml = video_xml("plugin.video.sample");
    let big = vec![0u8; FILE_CAP as usize + 1];
    let mut bytes = zip_bytes(&[
        ("plugin.video.sample/addon.xml", xml.as_bytes()),
        ("plugin.video.sample/big.bin", &big),
    ]);
    let honest = stage_zip(&bytes, Accept::UserChoice);
    assert!(honest.is_err());
    // Rewrite the big entry's uncompressed size to one byte in both the
    // local header and the central directory.
    let local = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, sig)| *sig == b"PK\x03\x04")
        .map(|(at, _)| at)
        .nth(1)
        .unwrap();
    bytes[local + 22..local + 26].copy_from_slice(&1u32.to_le_bytes());
    let central = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, sig)| *sig == b"PK\x01\x02")
        .map(|(at, _)| at)
        .nth(1)
        .unwrap();
    bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
    let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(archive.by_index(1).unwrap().size(), 1);
    assert!(stage_zip(&bytes, Accept::UserChoice).is_err());
}

#[test]
fn symlink_entries_are_skipped() {
    assert!(is_link_mode(Some(0o120777)));
    assert!(!is_link_mode(Some(0o100644)));
    assert!(!is_link_mode(None));
    let xml = video_xml("plugin.video.sample");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let file_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer
        .start_file("plugin.video.sample/addon.xml", file_options)
        .unwrap();
    writer.write_all(xml.as_bytes()).unwrap();
    writer
        .add_symlink(
            "plugin.video.sample/link.txt",
            "addon.xml",
            SimpleFileOptions::default(),
        )
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let staged = stage_zip(&bytes, Accept::UserChoice).unwrap();
    assert!(staged.files.iter().all(|(rel, _)| rel != "link.txt"));
}

#[test]
fn a_video_zip_lands_in_the_profile_and_is_not_replaced() {
    let root =
        std::env::temp_dir().join(format!("portal-addons-{}-{}", std::process::id(), unique()));
    let addons = Addons::new(
        root.join("profile"),
        root.join("programs"),
        root.join("staging"),
    );
    let xml = video_xml("plugin.video.sample");
    let bytes = zip_bytes(&[
        ("plugin.video.sample/addon.xml", xml.as_bytes()),
        ("plugin.video.sample/default.py", b"print(1)\n"),
    ]);
    let staged = stage_zip(&bytes, Accept::UserChoice).unwrap();
    let first = addons.commit(staged).unwrap();
    assert!(first.wrote);
    assert_eq!(first.kind, "video");
    let dest = root
        .join("profile")
        .join("addons")
        .join("plugin.video.sample")
        .join("default.py");
    assert_eq!(fs::read(dest).unwrap(), b"print(1)\n");
    let again = stage_zip(&bytes, Accept::UserChoice).unwrap();
    let second = addons.commit(again).unwrap();
    assert!(!second.wrote);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn installed_repos_read_names_from_disk() {
    let root =
        std::env::temp_dir().join(format!("portal-repos-{}-{}", std::process::id(), unique()));
    let dir = root.join("programs").join("repository.sample");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("addon.xml"), repo_xml()).unwrap();
    let addons = Addons::new(
        root.join("profile"),
        root.join("programs"),
        root.join("staging"),
    );
    let listed = addons.installed_repos().unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, "repository.sample");
    assert_eq!(listed.items[0].name, "Sample Repo");
    assert!(!listed.items[0].removable);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_profile_add_on_is_removed_and_everything_else_stays() {
    let root =
        std::env::temp_dir().join(format!("portal-remove-{}-{}", std::process::id(), unique()));
    let profile = root.join("profile");
    let programs = root.join("programs");
    let addons = Addons::new(&profile, &programs, root.join("staging"));
    let video = profile.join("addons").join("plugin.video.sample");
    let repo = profile.join("addons").join("repository.sample");
    let other = profile.join("addons").join("plugin.video.other");
    let shipped = programs.join("repository.xbmc.org");
    let packages = profile.join("addons").join("packages");
    fs::create_dir_all(&video).unwrap();
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::create_dir_all(&shipped).unwrap();
    fs::create_dir_all(&packages).unwrap();
    fs::write(video.join("addon.xml"), video_xml("plugin.video.sample")).unwrap();
    fs::write(video.join("default.py"), b"print(1)\n").unwrap();
    fs::write(repo.join("addon.xml"), repo_xml()).unwrap();
    // The other video add-on needs the module, so the module stays.
    fs::write(
            other.join("addon.xml"),
            video_xml("plugin.video.other").replace(
                "<extension",
                r#"<requires><import addon="script.module.resolver" version="1.0.0"/></requires><extension"#,
            ),
        )
        .unwrap();
    fs::write(
        shipped.join("addon.xml"),
        repo_xml().replace("repository.sample", "repository.xbmc.org"),
    )
    .unwrap();
    fs::write(packages.join("plugin.video.sample-1.2.3.zip"), b"zip").unwrap();
    fs::write(
        packages.join("plugin.video.sample.extra-1.0.0.zip"),
        b"keep",
    )
    .unwrap();
    fs::write(packages.join("plugin.video.other-1.0.0.zip"), b"other").unwrap();
    fs::write(packages.join("keep.zip"), b"keep").unwrap();
    let data = profile.join("userdata").join("addon_data");
    fs::create_dir_all(data.join("plugin.video.sample")).unwrap();
    fs::write(
        data.join("plugin.video.sample").join("settings.xml"),
        b"<settings/>",
    )
    .unwrap();
    fs::create_dir_all(data.join("script.module.resolver")).unwrap();
    fs::write(
        data.join("script.module.resolver").join("settings.xml"),
        b"<settings/>",
    )
    .unwrap();
    fs::create_dir_all(profile.join("addons").join("script.module.resolver")).unwrap();
    fs::write(
        profile
            .join("addons")
            .join("script.module.resolver")
            .join("addon.xml"),
        r#"<addon id="script.module.resolver" name="Resolver" version="1.0.0">
                <extension point="xbmc.python.module" library="lib"/>
              </addon>"#,
    )
    .unwrap();

    let removed = addons.remove_video("plugin.video.sample").unwrap();
    assert_eq!(removed.kind, "video");
    assert_eq!(removed.name, "Sample");
    assert!(!video.join("addon.xml").exists());
    assert!(!data.join("plugin.video.sample").exists());
    assert!(!packages.join("plugin.video.sample-1.2.3.zip").exists());
    assert_eq!(
        fs::read(packages.join("plugin.video.sample.extra-1.0.0.zip")).unwrap(),
        b"keep"
    );
    assert_eq!(
        fs::read(packages.join("plugin.video.other-1.0.0.zip")).unwrap(),
        b"other"
    );
    assert_eq!(fs::read(packages.join("keep.zip")).unwrap(), b"keep");
    assert!(other.join("addon.xml").is_file());
    assert!(repo.join("addon.xml").is_file());
    assert!(shipped.join("addon.xml").is_file());
    assert!(data
        .join("script.module.resolver")
        .join("settings.xml")
        .is_file());
    assert!(profile
        .join("addons")
        .join("script.module.resolver")
        .join("addon.xml")
        .is_file());

    let err = addons
        .remove_video("script.module.resolver")
        .unwrap_err()
        .to_string();
    assert!(err.contains("uses this module"));
    assert!(data
        .join("script.module.resolver")
        .join("settings.xml")
        .is_file());

    let err = addons
        .remove_repository("repository.xbmc.org")
        .unwrap_err()
        .to_string();
    assert!(err.contains("comes with Kodi"));
    assert!(shipped.join("addon.xml").is_file());

    let err = addons
        .remove_repository("plugin.video.other")
        .unwrap_err()
        .to_string();
    assert!(err.contains("video add-on"));
    assert!(other.join("addon.xml").is_file());

    let err = addons
        .remove_video("repository.sample")
        .unwrap_err()
        .to_string();
    assert!(err.contains("repository"));
    assert!(repo.join("addon.xml").is_file());

    let listed = addons.installed_repos().unwrap();
    let profile_repo = listed
        .items
        .iter()
        .find(|item| item.id == "repository.sample")
        .unwrap();
    assert!(profile_repo.removable);
    let shipped_repo = listed
        .items
        .iter()
        .find(|item| item.id == "repository.xbmc.org")
        .unwrap();
    assert!(!shipped_repo.removable);

    let removed_repo = addons.remove_repository("repository.sample").unwrap();
    assert_eq!(removed_repo.kind, "repository");
    assert!(!repo.exists());
    assert!(other.join("addon.xml").is_file());
    assert!(shipped.join("addon.xml").is_file());
    assert!(data
        .join("script.module.resolver")
        .join("settings.xml")
        .is_file());

    let err = addons
        .remove_video("plugin.video.sample")
        .unwrap_err()
        .to_string();
    assert!(err.contains("not installed"));
    let err = addons.remove_video("../secret").unwrap_err().to_string();
    assert!(err.contains("Unknown"));
    let err = addons.remove_video("packages").unwrap_err().to_string();
    assert!(err.contains("Unknown"));
    assert!(packages.join("keep.zip").is_file());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_half_removed_add_on_finishes_without_touching_another() {
    let root = std::env::temp_dir().join(format!(
        "portal-remove-retry-{}-{}",
        std::process::id(),
        unique()
    ));
    let profile = root.join("profile");
    let addons = Addons::new(&profile, root.join("programs"), root.join("staging"));
    let data = profile.join("userdata").join("addon_data");
    fs::create_dir_all(data.join("plugin.video.sample")).unwrap();
    fs::write(
        data.join("plugin.video.sample").join("settings.xml"),
        b"<settings/>",
    )
    .unwrap();
    fs::create_dir_all(data.join("script.module.resolver")).unwrap();
    fs::write(
        data.join("script.module.resolver").join("settings.xml"),
        b"<settings/>",
    )
    .unwrap();
    fs::create_dir_all(root.join("staging")).unwrap();
    fs::write(
        root.join("staging").join("remove-plugin.video.sample"),
        "video",
    )
    .unwrap();
    addons.remove_video("plugin.video.sample").unwrap();
    assert!(!data.join("plugin.video.sample").exists());
    assert!(data
        .join("script.module.resolver")
        .join("settings.xml")
        .is_file());
    assert!(!root
        .join("staging")
        .join("remove-plugin.video.sample")
        .exists());
    let _ = fs::remove_dir_all(&root);
}

/// A body that hands out each piece after its own wait.
struct Paced(Vec<(u64, Option<Vec<u8>>)>);

impl Chunks for Paced {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ()> {
        if self.0.is_empty() {
            return Ok(None);
        }
        let (wait, chunk) = self.0.remove(0);
        tokio::time::sleep(Duration::from_secs(wait)).await;
        Ok(chunk)
    }
}

#[tokio::test(start_paused = true)]
async fn a_slow_download_that_keeps_coming_finishes() {
    // Ten pieces twenty seconds apart take far longer than one stall.
    let mut body = Paced((0..10).map(|_| (20, Some(vec![7u8; 4]))).collect());
    let out = gather(&mut body, 1024, STALL_TIMEOUT, DOWNLOAD_TIMEOUT).await;
    assert_eq!(out.map(|out| out.len()), Some(40));
}

#[tokio::test(start_paused = true)]
async fn a_download_that_stalls_is_dropped() {
    let mut body = Paced(vec![(1, Some(vec![1u8; 4])), (31, Some(vec![2u8; 4]))]);
    assert_eq!(
        gather(&mut body, 1024, STALL_TIMEOUT, DOWNLOAD_TIMEOUT).await,
        None
    );
}

#[tokio::test(start_paused = true)]
async fn a_download_past_its_cap_or_its_total_time_is_dropped() {
    let mut body = Paced(vec![(0, Some(vec![1u8; 6])), (0, Some(vec![2u8; 6]))]);
    assert_eq!(
        gather(&mut body, 10, STALL_TIMEOUT, DOWNLOAD_TIMEOUT).await,
        None
    );
    let mut body = Paced((0..10).map(|_| (20, Some(vec![7u8; 4]))).collect());
    let total = Duration::from_secs(100);
    assert_eq!(gather(&mut body, 1024, STALL_TIMEOUT, total).await, None);
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[test]
fn zip_names_windows_cannot_hold_are_refused() {
    use std::path::Path;
    assert!(relative_zip_path(Path::new("plugin.video.example/addon.xml")).is_some());
    for bad in [
        "plugin.video.example/CON",
        "plugin.video.example/nul.txt",
        "plugin.video.example/com1.py",
        "plugin.video.example/a:stream",
        "plugin.video.example/trailing.",
        "plugin.video.example/trailing ",
        "plugin.video.example/what?.py",
    ] {
        assert!(relative_zip_path(Path::new(bad)).is_none(), "{bad}");
    }
    assert!(reserved_addon_id("packages"));
    assert!(reserved_addon_id("TEMP"));
    assert!(!reserved_addon_id("plugin.video.example"));
}

#[test]
fn the_index_marks_python_modules() {
    let packages = parse_index(sample_index(), "https://example.test/repo/").unwrap();
    let need = packages
        .iter()
        .find(|package| package.id == "script.module.need")
        .unwrap();
    assert!(need.module && !need.video);
    let sample = packages
        .iter()
        .find(|package| package.id == "plugin.video.sample")
        .unwrap();
    assert!(!sample.module);
}

#[test]
fn a_module_zip_is_accepted_as_a_module() {
    let xml = r#"<addon id="script.module.scraper" name="Scraper" version="1.0.0">
            <extension point="xbmc.python.module" library="lib" />
          </addon>"#;
    let bytes = zip_bytes(&[("script.module.scraper/addon.xml", xml.as_bytes())]);
    let staged = stage_zip(&bytes, Accept::UserChoice).unwrap();
    assert_eq!(staged.id, "script.module.scraper");
    assert_eq!(staged.kind, "module");
}

#[test]
fn a_repository_without_zip_marks_is_read_like_the_official_one() {
    let official = r#"<addon id="repository.xbmc.org" name="Kodi Add-on repository" version="3.4.0">
            <extension point="xbmc.addon.repository">
              <dir>
                <info compressed="true">https://mirrors.kodi.tv/addons/omega/addons.xml.gz</info>
                <checksum>https://mirrors.kodi.tv/addons/omega/addons.xml.gz.md5</checksum>
                <datadir>https://mirrors.kodi.tv/addons/omega</datadir>
              </dir>
            </extension>
          </addon>"#;
    assert!(read_feeds(official, None, true).is_err());
    let feeds = read_feeds(official, None, false).unwrap();
    assert_eq!(feeds[0].datadir, "https://mirrors.kodi.tv/addons/omega/");
}

#[test]
fn a_module_that_also_runs_as_a_program_stays_in_the_index() {
    let index = r#"<addons>
          <addon id="script.module.scraper" name="Scraper" version="1.0.39">
            <extension point="xbmc.python.pluginsource" library="lib/default.py"><provides>executable</provides></extension>
            <extension point="xbmc.service" library="lib/service.py" />
            <extension point="xbmc.python.module" library="lib" />
          </addon>
        </addons>"#;
    let packages = parse_index(index, "https://example.test/zips/").unwrap();
    assert_eq!(packages.len(), 1);
    assert!(packages[0].module && !packages[0].video);
}

#[test]
fn an_installed_module_can_be_removed() {
    let root =
        std::env::temp_dir().join(format!("portal-module-{}-{}", std::process::id(), unique()));
    let addons = Addons::new(
        root.join("profile"),
        root.join("programs"),
        root.join("staging"),
    );
    let xml = r#"<addon id="script.module.helper" name="Helper" version="1.0.0">
            <extension point="xbmc.python.module" library="lib" />
          </addon>"#;
    let bytes = zip_bytes(&[("script.module.helper/addon.xml", xml.as_bytes())]);
    assert!(
        addons
            .commit(stage_zip(&bytes, Accept::UserChoice).unwrap())
            .unwrap()
            .wrote
    );
    assert!(addons.video_removable("script.module.helper"));
    let removed = addons.remove_video("script.module.helper").unwrap();
    assert_eq!(removed.kind, "module");
    assert!(!root
        .join("profile")
        .join("addons")
        .join("script.module.helper")
        .exists());
    assert!(addons.remove_repository("script.module.helper").is_err());
    let _ = fs::remove_dir_all(&root);
}

fn temp_addons() -> (PathBuf, Addons) {
    let root =
        std::env::temp_dir().join(format!("portal-addons-{}-{}", std::process::id(), unique()));
    let addons = Addons::new(
        root.join("profile"),
        root.join("programs"),
        root.join("staging"),
    );
    (root, addons)
}

fn video_xml_at(id: &str, version: &str) -> String {
    video_xml(id).replace("version=\"1.2.3\"", &format!("version=\"{version}\""))
}

#[tokio::test]
async fn a_newer_version_in_a_repository_is_an_update() {
    let (root, addons) = temp_addons();
    let xml = video_xml("plugin.video.sample");
    let bytes = zip_bytes(&[("plugin.video.sample/addon.xml", xml.as_bytes())]);
    addons.commit(stage_zip(&bytes, Accept::UserChoice).unwrap()).unwrap();
    let repo = root.join("profile").join("addons").join("repository.sample");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("addon.xml"), repo_xml()).unwrap();
    assert_eq!(addons.profile_version("plugin.video.sample").as_deref(), Some("1.2.3"));

    // The repository's index, as if just read: the same version first.
    let same = parse_index(sample_index(), "https://all.example/zips/").unwrap();
    addons.store_cache("repository.sample", "Sample Repo", &same);
    assert!(addons.updates(true).await.unwrap().is_empty());

    let newer = sample_index().replacen("version=\"1.2.3\"", "version=\"1.10.0\"", 1);
    let packages = parse_index(&newer, "https://all.example/zips/").unwrap();
    addons.store_cache("repository.sample", "Sample Repo", &packages);
    let found = addons.updates(true).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "plugin.video.sample");
    assert_eq!(found[0].installed, "1.2.3");
    assert_eq!(found[0].available, "1.10.0");
    assert_eq!(found[0].repo, "repository.sample");
    assert_eq!(found[0].kind, "video");
    // Kept until asked to read again.
    addons.store_cache("repository.sample", "Sample Repo", &same);
    assert_eq!(addons.updates(false).await.unwrap().len(), 1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_update_replaces_the_folder_and_leaves_nothing_behind() {
    let (root, addons) = temp_addons();
    let old = video_xml("plugin.video.sample");
    let bytes = zip_bytes(&[
        ("plugin.video.sample/addon.xml", old.as_bytes()),
        ("plugin.video.sample/default.py", b"print(1)\n"),
        ("plugin.video.sample/retired.py", b"print(0)\n"),
    ]);
    addons.commit(stage_zip(&bytes, Accept::UserChoice).unwrap()).unwrap();
    let newer = video_xml_at("plugin.video.sample", "1.3.0");
    let bytes = zip_bytes(&[
        ("plugin.video.sample/addon.xml", newer.as_bytes()),
        ("plugin.video.sample/default.py", b"print(2)\n"),
    ]);
    let staged = stage_zip(&bytes, Accept::Listed("plugin.video.sample".into())).unwrap();
    addons.replace_staged(&staged).unwrap();
    let dir = root.join("profile").join("addons");
    assert_eq!(addons.profile_version("plugin.video.sample").as_deref(), Some("1.3.0"));
    assert_eq!(fs::read(dir.join("plugin.video.sample").join("default.py")).unwrap(), b"print(2)\n");
    assert!(!dir.join("plugin.video.sample").join("retired.py").exists());
    assert!(!dir.join("plugin.video.sample.portal-old").exists());
    // Nothing to replace: an update is not an install.
    let other = video_xml("plugin.video.other");
    let bytes = zip_bytes(&[("plugin.video.other/addon.xml", other.as_bytes())]);
    let staged = stage_zip(&bytes, Accept::UserChoice).unwrap();
    assert!(matches!(addons.replace_staged(&staged), Err(EngineError::NotFound)));
    assert!(!dir.join("plugin.video.other").exists());
    let _ = fs::remove_dir_all(&root);
}

/// Reads this PC's own Kodi profile and repositories and prints what has an
/// update. Read only. Run by hand: `cargo test -p portal-engine real_updates -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn real_updates() {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return;
    };
    let profile = PathBuf::from(appdata).join("Kodi");
    let programs = PathBuf::from(r"C:\Program Files\Kodi\addons");
    let staging = std::env::temp_dir().join("portal-real-updates");
    let addons = Addons::new(profile, programs, staging);
    for item in addons.updates(true).await.unwrap() {
        println!("{} {} -> {} ({})", item.id, item.installed, item.available, item.repo);
    }
}

#[tokio::test]
async fn an_install_drops_the_kept_list_of_updates() {
    let (root, addons) = temp_addons();
    *addons.updates.lock().unwrap() = Some((Instant::now(), Vec::new()));
    let xml = video_xml("plugin.video.sample");
    let bytes = zip_bytes(&[("plugin.video.sample/addon.xml", xml.as_bytes())]);
    addons.stage_and_commit(bytes, Accept::UserChoice).await.unwrap();
    assert!(addons.updates.lock().unwrap().is_none());
    *addons.updates.lock().unwrap() = Some((Instant::now(), Vec::new()));
    addons.remove_video("plugin.video.sample").unwrap();
    assert!(addons.updates.lock().unwrap().is_none());
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn repositories_that_cannot_be_read_are_not_up_to_date() {
    let (root, addons) = temp_addons();
    let repo = root.join("profile").join("addons").join("repository.sample");
    fs::create_dir_all(&repo).unwrap();
    // Its feeds are on hosts that do not exist.
    fs::write(repo.join("addon.xml"), repo_xml()).unwrap();
    assert!(addons.updates(true).await.is_err());
    assert!(addons.updates.lock().unwrap().is_none());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_module_older_than_the_update_needs_is_not_current() {
    let (root, addons) = temp_addons();
    let module = r#"<addon id="script.module.need" name="Need" version="1.0.0">
            <extension point="xbmc.python.module" library="lib"/>
          </addon>"#;
    let bytes = zip_bytes(&[("script.module.need/addon.xml", module.as_bytes())]);
    addons.commit(stage_zip(&bytes, Accept::UserChoice).unwrap()).unwrap();
    let newer = sample_index().replace(
        r#"<import addon="script.module.need" version="1.0.0"/>"#,
        r#"<import addon="script.module.need" version="1.2.0"/>"#,
    );
    let packages = parse_index(&newer, "https://all.example/zips/").unwrap();
    assert!(!addons.current_for(&packages, "plugin.video.sample", "script.module.need"));
    let same = parse_index(sample_index(), "https://all.example/zips/").unwrap();
    assert!(addons.current_for(&same, "plugin.video.sample", "script.module.need"));
    // The add-on being updated never counts as current.
    assert!(!addons.current_for(&same, "script.module.need", "script.module.need"));
    let _ = fs::remove_dir_all(&root);
}
