use super::*;

/// `index` counts list rows first, then buttons. A window with no packages
/// has a list length of 0, so the first choice is the first button.
/// Production answers use the visible button list directly. This maps the
/// same indexes for the catalog tests.
pub fn button_at(list_len: usize, index: i64, buttons: &[i64]) -> Option<i64> {
    let index = usize::try_from(index).ok()?;
    buttons.get(index.checked_sub(list_len)?).copied()
}

#[test]
fn property_names_come_from_skin_markup() {
    let xml = r#"
            <label>$INFO[Window.Property(example.heading)]</label>
            <label>$INFO[Window().Property(percent)]</label>
            <texture>$INFO[Window.Property(example.icon)]</texture>
        "#;
    let names = property_names(xml);
    assert!(names.iter().any(|n| n == "example.heading"));
    assert!(names.iter().any(|n| n == "percent"));
    assert!(names.iter().any(|n| n == "example.icon"));
}

#[test]
fn progress_skin_classifies_as_progress() {
    let dialog = parse_dialog(
            "progress_demo.xml",
            r#"<window><control type="progress" id="5000"/><label>$INFO[Window.Property(heading)]</label></window>"#,
        )
        .unwrap();
    assert_eq!(dialog.kind, PromptKind::Progress);
    assert!(dialog.properties.iter().any(|n| n == "heading"));
    assert!(dialog.control_ids.contains(&5000));
}

#[test]
fn playback_overlays_are_skipped() {
    assert!(skip_dialog("playnext.xml"));
    assert!(skip_dialog("skip_intro_aura.xml"));
    assert!(skip_dialog("ExamplePlayNext.xml"));
    assert!(skip_dialog("script-example-UpNext.xml"));
    assert!(skip_dialog("still_watching.xml"));
    assert!(skip_dialog("skin.example.SkipIntro.xml"));
    assert!(!skip_dialog("progress_example.xml"));
    assert!(!skip_dialog("confirm.xml"));
}

#[test]
fn title_prefers_heading_over_colors() {
    let values = vec![
        ("example.backgroundColor".into(), "FF000000".into()),
        ("example.heading".into(), "Example Host Auth".into()),
        ("percent".into(), "10".into()),
    ];
    assert_eq!(pick_title(&values), "Example Host Auth");
    let namespaced = vec![
        ("label".into(), "Plain".into()),
        ("other.addon.heading".into(), "Sign in".into()),
    ];
    assert_eq!(pick_title(&namespaced), "Sign in");
    assert_eq!(pick_percent(&values), Some(0.1));
}

#[test]
fn button_captions_are_not_titles() {
    let values = vec![
        ("ok_label".into(), "[B]OK[/B]".into()),
        ("heading".into(), "".into()),
        ("text".into(), "".into()),
    ];
    assert_eq!(pick_title(&values), "");
    assert!(is_button_caption_value("[B]OK[/B]"));
    assert!(is_button_caption_property("ok_label"));
}

#[test]
fn a_manager_window_without_dialog_in_its_name_is_kept() {
    let dialog = parse_dialog(
        "provider_packages.xml",
        r#"
            <window>
              <control type="list" id="2000">
                <itemlayout height="40">
                  <control type="label"><label>$INFO[ListItem.Label]</label></control>
                </itemlayout>
              </control>
              <control type="button" id="2002"><label>Install Package</label></control>
              <control type="button" id="2999"><label>Close</label></control>
            </window>
            "#,
    )
    .expect("manager window");
    assert_eq!(dialog.kind, PromptKind::Select);
    assert!(dialog.list.is_some());
    assert!(dialog.buttons.contains(&2002));
}

#[test]
fn a_custom_window_with_a_list_is_a_select() {
    let dialog = parse_dialog(
            "source_results.xml",
            r#"
            <window>
              <!-- <control type="list" id="9"></control> -->
              <control type="button" id="2051"><label>Back</label></control>
              <label>$INFO[Window().Property(example.title)]</label>
              <control type="list" id="2000">
                <orientation>Vertical</orientation>
                <itemlayout height="100">
                  <control type="image"><texture>$INFO[ListItem.Property(example.quality_icon)]</texture></control>
                  <control type="label">
                    <label>[COLOR $INFO[ListItem.Property(example.highlight)]]$INFO[ListItem.Property(example.count)] $INFO[ListItem.Property(example.name)][/COLOR]</label>
                    <visible>!String.Contains(ListItem.Property(example.source),UNCACHED)</visible>
                  </control>
                  <control type="label"><label>$INFO[ListItem.Property(example.size_label)] | $INFO[ListItem.Property(example.service)]</label></control>
                </itemlayout>
              </control>
              <control type="button" id="2001"><label>More</label></control>
            </window>
            "#,
        )
        .unwrap();
    assert_eq!(dialog.kind, PromptKind::Select);
    assert!(dialog.custom);
    let list = dialog.list.unwrap();
    assert_eq!(list.id, 2000);
    assert!(!list.horizontal);
    assert_eq!(
        list.fields,
        vec![
            "Property(example.name)",
            "Property(example.source)",
            "Property(example.size_label)",
            "Property(example.service)",
        ]
    );
    assert_eq!(dialog.buttons, vec![2051, 2001]);
    assert_eq!(button_at(0, 0, &dialog.buttons), Some(2051));
    assert_eq!(button_at(3, 4, &dialog.buttons), Some(2001));
    assert_eq!(button_at(3, 2, &dialog.buttons), None);
    let ok = parse_dialog(
        "ok.xml",
        r#"<window><label>$INFO[Window.Property(heading)]</label></window>"#,
    )
    .unwrap();
    assert_eq!(ok.kind, PromptKind::Ok);
    assert!(!ok.custom);
    assert!(ok.list.is_none());
    let plain = parse_dialog(
        "artwork.xml",
        r#"<window><label>$INFO[Window.Property(heading)]</label></window>"#,
    )
    .unwrap();
    assert_eq!(plain.kind, PromptKind::Ok);
    assert!(plain.custom);
}

#[test]
fn resolve_status_skin_is_progress() {
    let dialog = parse_dialog(
        "ResolveStatus.xml",
        r#"
            <window>
              <label>$INFO[Window().Property(title)]</label>
              <label>$INFO[Window().Property(status.main)]</label>
              <label>$INFO[Window().Property(status.sub)]</label>
              <control type="button" id="6"><label>Cancel</label></control>
            </window>
            "#,
    )
    .unwrap();
    assert_eq!(dialog.kind, PromptKind::Progress);
    let playback = parse_dialog(
        "sources_playback.xml",
        r#"
            <window>
              <label>Press BACK to Cancel</label>
              <control type="button" id="10"><label>Resume</label></control>
              <control type="button" id="11"><label>Start Over</label></control>
              <control type="button" id="12"><label>Cancel</label></control>
            </window>
            "#,
    )
    .unwrap();
    assert_eq!(playback.kind, PromptKind::Progress);
    assert!(playback.list.is_none());
    assert!(busy_overlay_file("source_progress.xml"));
    assert!(!busy_overlay_file("sources_results.xml"));
    assert!(!busy_overlay_file("playback_notifications.xml"));
    let values = vec![
        ("title".into(), "Lantern".into()),
        ("status.main".into(), "Resolving…".into()),
        ("status.sub".into(), "Cached copy".into()),
    ];
    assert_eq!(pick_title(&values), "Lantern");
    assert_eq!(pick_message(&values, "Lantern"), "Resolving…\nCached copy");
}

#[test]
fn textbox_body_is_the_message_and_the_heading_stays_the_title() {
    let dialog = parse_dialog(
        "progress_sign_in.xml",
        r#"
            <window>
              <!-- <control type="textbox" id="9"/> -->
              <control type="label">
                <label>$INFO[Window.Property(addon.heading)]</label>
              </control>
              <control type="list" id="50">
                <itemlayout>
                  <control type="label" id="9"><label>$INFO[ListItem.Label]</label></control>
                </itemlayout>
              </control>
              <control type="image" id="2020"/>
              <control type="textbox" id="2001"/>
              <control type="fadelabel"><id>12</id></control>
              <control type="progress" id="5000"/>
            </window>
            "#,
    )
    .unwrap();
    assert_eq!(dialog.kind, PromptKind::Progress);
    assert!(dialog.control_ids.contains(&5000));
    assert_eq!(dialog.text_ids, vec![2001, 12]);
    let values = vec![
        ("addon.heading".into(), "Example Host".into()),
        ("addon.label".into(), "Example Host".into()),
        ("percent".into(), "40".into()),
        (
            "Control.GetLabel(2001)".into(),
            "Visit https://auth.example/device\nCode ABCD".into(),
        ),
        ("Control.GetLabel(5000)".into(), "Progressbar".into()),
        (
            "Control.GetLabel(2020)".into(),
            "special://skin/media/icon.png".into(),
        ),
    ];
    assert_eq!(pick_title(&values), "Example Host");
    assert_eq!(
        pick_message(&values, "Example Host"),
        "Visit https://auth.example/device\nCode ABCD"
    );
}

#[test]
fn overlay_windows_are_found_by_shape_not_by_name() {
    let root = std::env::temp_dir().join(format!(
        "portal-overlay-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let skins = root
        .join("addons")
        .join("plugin.video.example")
        .join("resources")
        .join("skins")
        .join("Default")
        .join("1080i");
    fs::create_dir_all(&skins).unwrap();
    fs::write(
            skins.join("example_wait_zq.xml"),
            r#"<window><controls><control type="progress" id="5"/><control type="label" id="6"><label>$INFO[Window.Property(example.percent)]</label></control></controls></window>"#,
        )
        .unwrap();
    fs::write(
            skins.join("example_pick_zq.xml"),
            r#"<window><controls><control type="progress" id="5"/><control type="list" id="50"><itemlayout><control type="label"><info>ListItem.Label</info></control></itemlayout></control></controls></window>"#,
        )
        .unwrap();
    let _catalog = DialogCatalog::new(vec![root.clone()]);
    let keys = overlay_keys();
    assert!(keys.contains(&"Window.IsActive(example_wait_zq.xml)".to_string()));
    assert!(!keys.iter().any(|key| key.contains("example_pick_zq")));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_window_that_says_how_to_cancel_is_busy_whatever_its_name() {
    let busy = parse_dialog(
            "example_wait.xml",
            r#"<window><label>Press BACK to cancel</label><control type="button" id="10"><label>Resume</label></control></window>"#,
        )
        .unwrap();
    assert_eq!(busy.kind, PromptKind::Progress);
    let chooser = parse_dialog(
            "example_choose.xml",
            r#"<window><label>Pick one or press BACK to cancel</label><control type="list" id="50"><itemlayout><control type="label"><info>ListItem.Label</info></control></itemlayout></control></window>"#,
        )
        .unwrap();
    assert_ne!(chooser.kind, PromptKind::Progress);
}
