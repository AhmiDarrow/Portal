use super::*;

fn fixture(xml: &str, saved: &str, po: &str) -> (Settings, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "portal-settings-{}-{}",
        std::process::id(),
        unique()
    ));
    let addon = root
        .join("profile")
        .join("addons")
        .join("plugin.video.demo");
    fs::create_dir_all(
        addon
            .join("resources")
            .join("language")
            .join("resource.language.en_gb"),
    )
    .unwrap();
    fs::write(addon.join("resources").join("settings.xml"), xml).unwrap();
    if !po.is_empty() {
        fs::write(
            addon
                .join("resources")
                .join("language")
                .join("resource.language.en_gb")
                .join("strings.po"),
            po,
        )
        .unwrap();
    }
    if !saved.is_empty() {
        let data = root
            .join("profile")
            .join("userdata")
            .join("addon_data")
            .join("plugin.video.demo");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("settings.xml"), saved).unwrap();
    }
    let settings = Settings::new(root.join("profile"), root.join("programs"));
    (settings, root)
}

fn unique() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn eq_counts_the_separator_and_system_stays_visible() {
    let xml = r#"<settings>
            <category label="General">
                <setting id="toggle" type="bool" label="Toggle" default="true"/>
                <setting type="lsep" label="Break"/>
                <setting id="shown" type="text" label="Shown" visible="eq(-2,true)"/>
                <setting id="system" type="text" label="System" visible="System.HasAddon(plugin.video.demo)"/>
                <setting id="hidden" type="text" label="Hidden" visible="false"/>
            </category>
        </settings>"#;
    let (settings, root) = fixture(xml, "", "");
    let form = settings.form("plugin.video.demo").unwrap();
    let labels: Vec<_> = form.categories[0]
        .fields
        .iter()
        .map(|field| field.label.as_str())
        .collect();
    assert!(labels.contains(&"Shown"));
    assert!(labels.contains(&"System"));
    assert!(!labels.contains(&"Hidden"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn secrets_stay_blank_and_an_empty_write_keeps_the_saved_value() {
    let xml = r#"<settings>
            <category label="Keys">
                <setting id="token" type="text" label="API Key" default=""/>
            </category>
        </settings>"#;
    let saved = r#"<settings version="2"><setting id="token">SECRETTOKEN</setting></settings>"#;
    let (settings, root) = fixture(xml, saved, "");
    let form = settings.form("plugin.video.demo").unwrap();
    let json = serde_json::to_string(&form).unwrap();
    assert!(!json.contains("SECRETTOKEN"));
    assert_eq!(form.categories[0].fields[0].value, "");
    assert!(form.categories[0].fields[0].secret);
    settings
        .write_value("plugin.video.demo", "token", "")
        .unwrap();
    let kept = fs::read_to_string(
        root.join("profile")
            .join("userdata")
            .join("addon_data")
            .join("plugin.video.demo")
            .join("settings.xml"),
    )
    .unwrap();
    assert!(kept.contains("SECRETTOKEN"));
    settings
        .write_value("plugin.video.demo", "token", "next-value")
        .unwrap();
    let changed = fs::read_to_string(
        root.join("profile")
            .join("userdata")
            .join("addon_data")
            .join("plugin.video.demo")
            .join("settings.xml"),
    )
    .unwrap();
    assert!(changed.contains("next-value"));
    assert!(!changed.contains("SECRETTOKEN"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn version_two_text_is_read_and_actions_are_gated() {
    let xml = r#"<settings>
            <category label="Actions">
                <setting id="open" type="action" label="Open" action="RunPlugin(plugin://plugin.video.demo/)"/>
                <setting id="reset" type="action" label="Reset Function Cache" action="RunPlugin(plugin://plugin.video.demo/?mode=reset)"/>
                <setting id="name" type="text" label="Name" default=""/>
            </category>
        </settings>"#;
    let saved = r#"<settings version="2"><setting id="name">Kept</setting></settings>"#;
    let (settings, root) = fixture(xml, saved, "");
    let form = settings.form("plugin.video.demo").unwrap();
    let name = form.categories[0]
        .fields
        .iter()
        .find(|field| field.id.as_deref() == Some("name"))
        .unwrap();
    assert_eq!(name.value, "Kept");
    assert_eq!(
        settings
            .action_builtin("plugin.video.demo", "open")
            .unwrap(),
        "RunPlugin(plugin://plugin.video.demo/)"
    );
    assert_eq!(
        settings
            .action_builtin("plugin.video.demo", "reset")
            .unwrap(),
        "RunPlugin(plugin://plugin.video.demo/?mode=reset)"
    );
    assert!(settings
        .write_value("plugin.video.demo", "open", "nope")
        .is_err());
    assert!(settings.form("../evil").is_err());
    assert!(settings.form("plugin/video").is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_legacy_enum_saves_its_position_and_options_show_their_labels() {
    let xml = r#"<settings>
            <category label="General">
                <setting id="quality" type="enum" label="Quality" lvalues="32001|32002|32003" default="0"/>
                <setting id="limit" type="enum" label="Limit" values="Ten|Twenty" entries="10|20" default="10"/>
                <setting id="kind" type="labelenum" label="Kind" values="Movies|Shows" default="Movies"/>
                <setting id="shown" type="text" label="Shown" visible="eq(-3,2)"/>
                <setting id="mode" type="integer" label="Mode">
                    <default>0</default>
                    <constraints><options>
                        <option label="32001">0</option>
                        <option label="32002">1</option>
                    </options></constraints>
                    <control type="list" format="string"/>
                </setting>
            </category>
        </settings>"#;
    let po = "msgctxt \"#32001\"\nmsgid \"Low\"\nmsgstr \"\"\n\nmsgctxt \"#32002\"\nmsgid \"Medium\"\nmsgstr \"\"\n\nmsgctxt \"#32003\"\nmsgid \"High\"\nmsgstr \"\"\n";
    let saved = r#"<settings version="2"><setting id="quality">2</setting><setting id="mode">1</setting></settings>"#;
    let (settings, root) = fixture(xml, saved, po);
    let form = settings.form("plugin.video.demo").unwrap();
    let fields = &form.categories[0].fields;
    let find = |id: &str| {
        fields
            .iter()
            .find(|field| field.id.as_deref() == Some(id))
            .unwrap()
    };
    let pairs = |id: &str| {
        find(id)
            .options
            .iter()
            .map(|option| (option.value.clone(), option.label.clone()))
            .collect::<Vec<_>>()
    };
    let pair = |value: &str, label: &str| (value.to_string(), label.to_string());
    assert_eq!(
        pairs("quality"),
        vec![pair("0", "Low"), pair("1", "Medium"), pair("2", "High")]
    );
    assert_eq!(find("quality").value, "2");
    assert_eq!(
        pairs("limit"),
        vec![pair("10", "Ten"), pair("20", "Twenty")]
    );
    assert_eq!(find("limit").value, "10");
    assert_eq!(
        pairs("kind"),
        vec![pair("Movies", "Movies"), pair("Shows", "Shows")]
    );
    assert_eq!(pairs("mode"), vec![pair("0", "Low"), pair("1", "Medium")]);
    assert_eq!(find("mode").value, "1");
    assert!(fields.iter().any(|field| field.label == "Shown"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_wrapped_condition_with_accents_is_still_read() {
    let xml = r#"<settings>
            <category label="General">
                <setting id="name" type="text" label="Name" default=""/>
                <setting id="match" type="text" label="Match" visible="(is(name,caf&#233;))"/>
                <setting id="other" type="text" label="Other" visible="(is(name,th&#233;))"/>
            </category>
        </settings>"#;
    let saved = "<settings version=\"2\"><setting id=\"name\">caf\u{e9}</setting></settings>";
    let (settings, root) = fixture(xml, saved, "");
    let form = settings.form("plugin.video.demo").unwrap();
    let labels: Vec<_> = form.categories[0]
        .fields
        .iter()
        .map(|field| field.label.as_str())
        .collect();
    assert!(labels.contains(&"Match"));
    assert!(!labels.contains(&"Other"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn group_titles_and_kodi_markup_become_plain_labels() {
    let xml = r#"<settings version="1">
            <section id="plugin.video.demo">
                <category id="accounts" label="32346">
                    <group id="1" label="40057">
                        <setting id="serviceA.enable" type="boolean" label="32081" default="true"/>
                    </group>
                    <group id="2" label="40058">
                        <setting id="serviceB.enable" type="boolean" label="[B][COLOR white]Enable[/COLOR][/B]" default="true"/>
                        <setting id="serviceBAuthorize" type="action" label="40121">
                            <data>RunPlugin(plugin://plugin.video.demo/?action=auth)</data>
                        </setting>
                    </group>
                    <group id="3" label="[CR]"/>
                    <group id="4">
                        <setting id="bare" type="boolean" label="Bare" default="true"/>
                    </group>
                </category>
                <category id="nav" label="[B][COLOR crimson]Accounts[/COLOR][/B]">
                    <setting type="lsep" label="32338"/>
                    <setting id="provider.example" type="boolean" label="32101"/>
                    <setting id="from.addon" type="text" label="$ADDON[plugin.video.other 40058]"/>
                    <setting id="mixed" type="text" label="X$NUMBER[4]Y"/>
                </category>
                <category id="offsets" label="Offsets">
                    <setting id="toggle" type="bool" label="Toggle" default="true"/>
                    <setting type="lsep" label="[CR]"/>
                    <setting id="shown" type="text" label="Shown" visible="eq(-2,true)"/>
                    <setting id="inside" type="bool" label="Inside" default="true"/>
                    <group label="Section">
                        <setting id="nested" type="bool" label="Nested" default="false"/>
                    </group>
                    <setting id="after" type="text" label="After" visible="eq(-2,true)"/>
                </category>
            </section>
        </settings>"#;
    let po = "\
msgctxt \"#32346\"\nmsgid \"Services(Hosts)\"\nmsgstr \"\"\n\n\
msgctxt \"#40057\"\nmsgid \"Service A\"\nmsgstr \"\"\n\n\
msgctxt \"#40058\"\nmsgid \"Service B\"\nmsgstr \"\"\n\n\
msgctxt \"#32081\"\nmsgid \"Enable\"\nmsgstr \"\"\n\n\
msgctxt \"#40121\"\nmsgid \"Authorization\"\nmsgstr \"\"\n\n\
msgctxt \"#32338\"\nmsgid \"[B][COLOR lightskyblue]Lists[/COLOR][/B]\"\nmsgstr \"\"\n\n\
msgctxt \"#32101\"\nmsgid \"$NUMBER[7]X\"\nmsgstr \"\"\n";
    let (settings, root) = fixture(xml, "", po);
    let form = settings.form("plugin.video.demo").unwrap();
    let labels = |index: usize| {
        form.categories[index]
            .fields
            .iter()
            .map(|field| field.label.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(form.categories[0].label, "Services(Hosts)");
    assert_eq!(
        labels(0),
        vec![
            "Service A",
            "Enable",
            "Service B",
            "Enable",
            "Authorization",
            "Bare",
        ]
    );
    assert_eq!(form.categories[0].fields[0].kind, FieldKind::Header);
    assert!(form.categories[0].fields[0].id.is_none());
    assert_eq!(form.categories[0].fields[2].kind, FieldKind::Header);
    assert_eq!(form.categories[1].label, "Accounts");
    assert_eq!(
        labels(1),
        vec!["Lists", "7X", "Service B", "X4Y"]
    );
    assert_eq!(form.categories[1].fields[0].kind, FieldKind::Header);
    assert_eq!(form.categories[2].label, "Offsets");
    assert_eq!(
        labels(2),
        vec!["Toggle", "Shown", "Inside", "Section", "Nested", "After"]
    );
    assert_eq!(form.categories[2].fields[3].kind, FieldKind::Header);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_saved_file_that_cannot_be_read_is_not_replaced() {
    let xml = r#"<settings>
            <category label="General">
                <setting id="name" type="text" label="Name" default=""/>
            </category>
        </settings>"#;
    let (settings, root) = fixture(xml, "", "");
    let data = root
        .join("profile")
        .join("userdata")
        .join("addon_data")
        .join("plugin.video.demo");
    fs::create_dir_all(&data).unwrap();
    let saved = data.join("settings.xml");
    let bytes = b"<settings version=\"2\"><setting id=\"keep\">caf\xe9</setting></settings>";
    fs::write(&saved, bytes).unwrap();
    assert!(settings
        .write_value("plugin.video.demo", "name", "next")
        .is_err());
    assert_eq!(fs::read(&saved).unwrap(), bytes);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_saved_file_cut_off_mid_write_is_not_replaced() {
    let xml = r#"<settings>
            <category label="General">
                <setting id="name" type="text" label="Name" default=""/>
            </category>
        </settings>"#;
    let saved_path = |root: &Path| {
        root.join("profile")
            .join("userdata")
            .join("addon_data")
            .join("plugin.video.demo")
            .join("settings.xml")
    };
    let whole = "<settings version=\"2\"><setting id=\"keep\">1</setting><setting id=\"more\">2</setting></settings>";
    for cut in [
        "<settings version=\"2\"><setting id=\"keep\">1</setting>",
        "<settings version=\"2\"><setting id=\"keep\">1</setting><setting id=\"mo",
        "<settings version=\"2\"><setting id=\"keep\">1",
    ] {
        let (settings, root) = fixture(xml, cut, "");
        assert!(settings
            .write_value("plugin.video.demo", "name", "next")
            .is_err());
        assert_eq!(fs::read_to_string(saved_path(&root)).unwrap(), cut);
        let _ = fs::remove_dir_all(root);
    }
    // A whole file is still written, and an empty one starts fresh.
    for (saved, kept) in [(whole, true), ("  ", false)] {
        let (settings, root) = fixture(xml, saved, "");
        settings
            .write_value("plugin.video.demo", "name", "next")
            .unwrap();
        let text = fs::read_to_string(saved_path(&root)).unwrap();
        assert!(text.contains("next"));
        assert_eq!(text.contains("more"), kept);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn addons_are_found_by_name() {
    let root =
        std::env::temp_dir().join(format!("portal-named-{}-{}", std::process::id(), unique()));
    let plain = r#"<settings><category label="Main"><setting id="quality" type="text" default=""/></category></settings>"#;
    let module = root
        .join("profile")
        .join("addons")
        .join("script.module.first");
    let other = root.join("programs").join("plugin.video.example");
    for (dir, name) in [(&module, "First Module"), (&other, "Example Videos")] {
        fs::create_dir_all(dir.join("resources")).unwrap();
        fs::write(dir.join("resources").join("settings.xml"), plain).unwrap();
        fs::write(
            dir.join("addon.xml"),
            format!(r#"<addon id="x" name="[B]{name}[/B]" version="1.0.0"></addon>"#),
        )
        .unwrap();
    }
    let settings = Settings::new(root.join("profile"), root.join("programs"));
    assert_eq!(
        settings.addon_named("FIRST MODULE").as_deref(),
        Some("script.module.first")
    );
    assert_eq!(
        settings.addon_named("ExampleVideos").as_deref(),
        Some("plugin.video.example")
    );
    assert_eq!(settings.addon_named("Missing"), None);
    assert_eq!(settings.addon_named(""), None);
    let _ = fs::remove_dir_all(root);
}
