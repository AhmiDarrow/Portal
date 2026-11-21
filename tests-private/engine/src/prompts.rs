use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn only_a_whole_word_pin_hides_a_prompt_value() {
    assert!(secret_title("Enter PIN"));
    assert!(secret_title("PIN code:"));
    assert!(secret_title("Account password"));
    assert!(!secret_title("Your opinion"));
    assert!(!secret_title("Spinoff title"));
}

#[test]
fn classify_window_lists_every_dialog_portal_mirrors() {
    let mapped = [
        (10100, PromptKind::Yesno),
        (10101, PromptKind::Progress),
        (10103, PromptKind::Text),
        (10109, PromptKind::Text),
        (10126, PromptKind::Text),
        (10138, PromptKind::Progress),
        (10147, PromptKind::Ok),
        (10160, PromptKind::Progress),
        (12000, PromptKind::Select),
        (12002, PromptKind::Ok),
    ];
    for (id, kind) in mapped {
        assert_eq!(classify_window(id), Some(kind), "{id}");
    }
    for id in [
        10000, 10025, 10502, 10107, 10151, 12005, 12006, 12900, 12901, 10140, 10106, 10145,
    ] {
        assert_eq!(classify_window(id), None, "{id}");
    }
}

#[test]
fn progress_bar_label_is_a_fraction() {
    assert_eq!(progress_fraction("40"), Some(0.4));
    assert_eq!(progress_fraction(""), None);
    assert_eq!(progress_fraction("nope"), None);
}

#[test]
fn blank_yesno_and_ok_are_hidden_progress_is_kept() {
    let blank = prompt("w10100", PromptKind::Yesno, "", "", Vec::new(), true, None);
    assert!(prompt_copy_is_blank(&blank));
    let ok = prompt("w12002", PromptKind::Ok, "", "", Vec::new(), false, None);
    assert!(prompt_copy_is_blank(&ok));
    let asked = prompt(
        "w10100",
        PromptKind::Yesno,
        "Resume?",
        "",
        Vec::new(),
        true,
        None,
    );
    assert!(!prompt_copy_is_blank(&asked));
    let working = busy("w10101", true);
    assert!(!prompt_copy_is_blank(&working));
    assert_eq!(working.title, "Working");
    let progress = prompt(
        "w10101",
        PromptKind::Progress,
        "",
        "",
        Vec::new(),
        true,
        Some(0.2),
    );
    assert!(!prompt_copy_is_blank(&progress));
    // Skin progress with no labels still uses a public "Working" title.
    let skin_busy = prompt(
        "skin:progress.xml",
        PromptKind::Progress,
        "Working",
        "",
        Vec::new(),
        true,
        None,
    );
    assert!(!prompt_copy_is_blank(&skin_busy));
    assert_eq!(skin_busy.title, "Working");
}

#[tokio::test]
async fn reads_yesno_from_a_fake_socket() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        answer_yesno(socket).await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    let prompt = prompts.read().await.unwrap().expect("yes/no");
    assert_eq!(
        prompt,
        Prompt {
            id: "w10100-1".into(),
            kind: PromptKind::Yesno,
            title: "Replace?".into(),
            message: "This file exists.".into(),
            options: Vec::new(),
            cancellable: true,
            progress: None,
        }
    );
    drop(prompts);
    client.close().await;
}

#[tokio::test]
async fn two_readers_close_add_on_settings_once() {
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let window = Arc::new(AtomicI64::new(10140));
    let backs = Arc::new(AtomicUsize::new(0));
    let (seen_window, seen_backs) = (window.clone(), backs.clone());
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve(socket, move |method| match method {
            "GUI.GetProperties" => {
                json!({"currentwindow": {"id": seen_window.load(Ordering::SeqCst)}})
            }
            "Input.Back" => {
                seen_backs.fetch_add(1, Ordering::SeqCst);
                seen_window.store(10000, Ordering::SeqCst);
                json!("OK")
            }
            "XBMC.GetInfoBooleans" | "XBMC.GetInfoLabels" => json!({}),
            other => panic!("unexpected method {other}"),
        })
        .await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    let (first, second) = tokio::join!(prompts.read(), prompts.read());
    assert!(first.unwrap().is_none());
    assert!(second.unwrap().is_none());
    assert_eq!(backs.load(Ordering::SeqCst), 1);
    assert_eq!(window.load(Ordering::SeqCst), 10000);
    drop(prompts);
    client.close().await;
}

#[test]
fn a_resolve_window_is_left_alone_while_another_skin_is_empty() {
    assert!(resolve_skin_open(
        &json!({"Window.IsActive(source_progress.xml)": true})
    ));
    assert!(resolve_skin_open(
        &json!({"Window.IsActive(ResolveStatus.xml)": true})
    ));
    assert!(!resolve_skin_open(&json!({
        "Window.IsActive(source_progress.xml)": false,
        "Window.IsActive(ResolveStatus.xml)": false
    })));
}

#[test]
fn a_leftover_list_is_not_answered_while_a_resolve_window_is_open() {
    let scraping = json!({ "Window.IsActive(source_progress.xml)": true });
    assert!(answer_would_hit_resolve("source_results.xml", &scraping));
    assert!(!answer_would_hit_resolve("source_progress.xml", &scraping));
    let resolve = json!({ "Window.IsActive(ResolveStatus.xml)": true });
    assert!(answer_would_hit_resolve("source_results.xml", &resolve));
    assert!(!answer_would_hit_resolve(
        "source_results.xml",
        &json!({
            "Window.IsActive(source_progress.xml)": false,
            "Window.IsActive(ResolveStatus.xml)": false
        })
    ));
}

#[test]
fn a_focused_source_list_is_answered_while_the_scrape_window_is_still_marked_active() {
    let scraping = json!({ "Window.IsActive(source_progress.xml)": true });
    assert!(!select_would_cancel_resolve(
        true,
        "source_results.xml",
        &scraping
    ));
    assert!(select_would_cancel_resolve(
        false,
        "source_results.xml",
        &scraping
    ));
    let quiet = json!({
        "Window.IsActive(source_progress.xml)": false,
        "Window.IsActive(ResolveStatus.xml)": false
    });
    assert!(!select_would_cancel_resolve(
        false,
        "source_results.xml",
        &quiet
    ));
}

#[test]
fn list_rows_name_first_and_skip_placeholders() {
    let values = vec![
        "[B]Show.S01E01.1080p[/B]".to_string(),
        "None".to_string(),
        "2.1 GB".to_string(),
        "".to_string(),
        "Service".to_string(),
    ];
    assert_eq!(
        row_text(&values, 0),
        "Show.S01E01.1080p \u{2014} 2.1 GB | Service"
    );
    assert_eq!(row_text(&["".to_string()], 4), "Item 5");
}

#[tokio::test]
async fn a_source_list_window_reads_as_a_cancellable_select() {
    read_source_list(false).await;
}

#[tokio::test]
async fn a_source_list_wins_over_a_scrape_window_still_active() {
    read_source_list(true).await;
}

/// `scraping` leaves the add-on's progress window active under its list,
/// as a hidden Kodi does when the window's closing animation never ends.
async fn read_source_list(scraping: bool) {
    let root = std::env::temp_dir().join(format!(
        "portal-prompts-{}-{}",
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
    std::fs::create_dir_all(&skins).unwrap();
    std::fs::write(
            skins.join("source_results.xml"),
            r#"<window>
              <label>$INFO[Window().Property(example.title)]</label>
              <control type="list" id="2000"><itemlayout>
                <control type="label"><label>$INFO[ListItem.Property(example.name)]</label></control>
                <control type="label"><label>$INFO[ListItem.Property(example.size_label)] | $INFO[ListItem.Property(example.service)]</label></control>
              </itemlayout></control>
            </window>"#,
        )
        .unwrap();
    if scraping {
        std::fs::write(
                skins.join("source_progress.xml"),
                r#"<window><control type="progress" id="5000"/><label>$INFO[Window.Property(example.status)]</label></window>"#,
            )
            .unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve(socket, move |method| match method {
            "GUI.GetProperties" => json!({"currentwindow": {"id": 13001}}),
            "XBMC.GetInfoBooleans" => json!({
                "Window.IsActive(source_progress.xml)": scraping,
                "Window.IsActive(source_results.xml)": true
            }),
            "XBMC.GetInfoLabels" => json!({
                "Window.Property(example.status)": "Scraping",
                "Window.Property(example.title)": "Pilot",
                "Container(2000).NumItems": "2",
                "Container(2000).ListItemAbsolute(0).Property(example.name)": "Show.S01E01.1080p",
                "Container(2000).ListItemAbsolute(0).Property(example.size_label)": "2.1 GB",
                "Container(2000).ListItemAbsolute(0).Property(example.service)": "Service",
                "Container(2000).ListItemAbsolute(1).Property(example.name)": "Show.S01E01.720p",
                "Container(2000).ListItemAbsolute(1).Property(example.size_label)": "NA"
            }),
            other => panic!("unexpected method {other}"),
        })
        .await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::with_addon_roots(client.clone(), vec![root.clone()]);
    let prompt = prompts.read().await.unwrap().expect("source list");
    assert_eq!(prompt.id, "skin:source_results.xml-1");
    assert_eq!(prompt.kind, PromptKind::Select);
    assert_eq!(prompt.title, "Pilot");
    assert!(prompt.cancellable);
    assert_eq!(
        prompt.options,
        vec![
            "Show.S01E01.1080p \u{2014} 2.1 GB | Service".to_string(),
            "Show.S01E01.720p".to_string(),
        ]
    );
    drop(prompts);
    client.close().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A Kodi select dialog whose key presses take effect one label read
/// late, as a press queued for the next frame does.
struct LateSelect {
    window: i64,
    title: String,
    rows: i64,
    row: i64,
    queued: Vec<i64>,
    selected: Option<i64>,
}

fn late_select(rows: i64) -> Arc<Mutex<LateSelect>> {
    Arc::new(Mutex::new(LateSelect {
        window: 12000,
        title: "Choose a source".into(),
        rows,
        row: 0,
        queued: Vec::new(),
        selected: None,
    }))
}

async fn serve_late_select(port_state: Arc<Mutex<LateSelect>>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve(socket, move |method| {
            let mut kodi = port_state.lock().unwrap();
            match method {
                "GUI.GetProperties" => json!({"currentwindow": {"id": kodi.window}}),
                "XBMC.GetInfoBooleans" => json!({}),
                "XBMC.GetInfoLabels" => {
                    let mut labels = serde_json::Map::new();
                    labels.insert("Control.GetLabel(1)".into(), json!(kodi.title));
                    labels.insert("Container(3).NumItems".into(), json!(kodi.rows.to_string()));
                    labels.insert("Container(6).NumItems".into(), json!("0"));
                    labels.insert(
                        "Container(3).CurrentItem".into(),
                        json!((kodi.row + 1).to_string()),
                    );
                    for item in 0..kodi.rows {
                        labels.insert(
                            format!("Container(3).ListItemAbsolute({item}).Label"),
                            json!(format!("Source {item}")),
                        );
                    }
                    // Queued presses land after this read answered.
                    let moves: i64 = kodi.queued.drain(..).sum();
                    kodi.row = (kodi.row + moves).clamp(0, kodi.rows - 1);
                    Value::Object(labels)
                }
                "Input.Down" => {
                    kodi.queued.push(1);
                    json!("OK")
                }
                "Input.Up" => {
                    kodi.queued.push(-1);
                    json!("OK")
                }
                "Input.Select" => {
                    let moves: i64 = kodi.queued.drain(..).sum();
                    kodi.row = (kodi.row + moves).clamp(0, kodi.rows - 1);
                    kodi.selected = Some(kodi.row);
                    json!("OK")
                }
                "Input.Back" => {
                    kodi.window = 10000;
                    json!("OK")
                }
                other => panic!("unexpected method {other}"),
            }
        })
        .await;
    });
    port
}

fn submit(id: &str, index: i32) -> PromptAnswer {
    PromptAnswer {
        id: id.into(),
        action: PromptAction::Submit,
        text: None,
        index: Some(index),
    }
}

#[tokio::test]
async fn a_select_lands_on_the_asked_row_when_kodi_acts_a_frame_late() {
    let kodi = late_select(6);
    let port = serve_late_select(kodi.clone()).await;
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    prompts.submit_select(&submit("w12000", 4)).await.unwrap();
    assert_eq!(kodi.lock().unwrap().selected, Some(4));
    {
        let mut kodi = kodi.lock().unwrap();
        kodi.selected = None;
    }
    prompts.submit_select(&submit("w12000", 1)).await.unwrap();
    assert_eq!(kodi.lock().unwrap().selected, Some(1));
    // A row past the end is refused before any key is pressed.
    assert!(prompts.submit_select(&submit("w12000", 6)).await.is_err());
    assert_eq!(kodi.lock().unwrap().selected, Some(1));
    drop(prompts);
    client.close().await;
}

#[tokio::test]
async fn cancel_closes_a_select_list() {
    let kodi = late_select(3);
    let port = serve_late_select(kodi.clone()).await;
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    let shown = prompts.read().await.unwrap().expect("select").id;
    let cancel = PromptAnswer {
        id: shown,
        action: PromptAction::Cancel,
        text: None,
        index: None,
    };
    prompts.answer(&cancel).await.unwrap();
    assert_eq!(kodi.lock().unwrap().window, 10000);
    assert_eq!(kodi.lock().unwrap().selected, None);
    drop(prompts);
    client.close().await;
}

#[test]
fn showing_numbers_split_off_prompt_ids() {
    assert_eq!(split_showing("w12000-7"), ("w12000", Some(7)));
    assert_eq!(split_showing("w12000"), ("w12000", None));
    assert_eq!(
        split_showing("skin:source_results.xml-3"),
        ("skin:source_results.xml", Some(3))
    );
    assert_eq!(
        split_showing("skin:my-dialog.xml"),
        ("skin:my-dialog.xml", None)
    );
    assert_eq!(split_showing("w12000-"), ("w12000-", None));
    assert_eq!(split_showing("-4"), ("-4", None));
}

#[tokio::test]
async fn each_showing_of_a_window_gets_its_own_id() {
    let kodi = late_select(2);
    let port = serve_late_select(kodi.clone()).await;
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    let id = |prompt: Option<Prompt>| prompt.expect("select").id;
    assert_eq!(id(prompts.read().await.unwrap()), "w12000-1");
    assert_eq!(id(prompts.read().await.unwrap()), "w12000-1");
    // The next select in the same window asks something else.
    kodi.lock().unwrap().title = "Choose a file".into();
    assert_eq!(id(prompts.read().await.unwrap()), "w12000-2");
    // An answer meant for the first one does not reach the second.
    let stale = prompts.answer(&submit("w12000-1", 0)).await;
    assert!(matches!(stale, Err(EngineError::BadRequest(_))));
    assert_eq!(kodi.lock().unwrap().selected, None);
    assert!(prompts.showing_current("w12000", Some(2)));
    assert!(prompts.showing_current("w12000", None));
    // The same question asked again after an answer is a new showing.
    prompts.note_answered("w12000");
    assert_eq!(id(prompts.read().await.unwrap()), "w12000-3");
    // So is one that closed and opened again.
    kodi.lock().unwrap().window = 10000;
    assert!(prompts.read().await.unwrap().is_none());
    kodi.lock().unwrap().window = 12000;
    assert_eq!(id(prompts.read().await.unwrap()), "w12000-4");
    drop(prompts);
    client.close().await;
}

#[tokio::test]
async fn a_modal_dialog_left_closing_reads_as_stuck() {
    // (visible, active): closing, open, and none.
    let state = Arc::new(Mutex::new((true, false)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let kodi = state.clone();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve(socket, move |method| match method {
            "XBMC.GetInfoBooleans" => {
                let (visible, active) = *kodi.lock().unwrap();
                json!({
                    "System.HasVisibleModalDialog": visible,
                    "System.HasActiveModalDialog": active
                })
            }
            "GUI.GetProperties" => json!({"currentwindow": {"id": 10100}}),
            other => panic!("unexpected method {other}"),
        })
        .await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    assert!(prompts.modal_close_stuck().await);
    *state.lock().unwrap() = (true, true);
    assert!(!prompts.modal_close_stuck().await);
    *state.lock().unwrap() = (false, false);
    assert!(!prompts.modal_close_stuck().await);
    drop(prompts);
    client.close().await;
}

#[tokio::test]
async fn yes_walks_to_the_left_button_and_selects_only_there() {
    struct YesNo {
        window: i64,
        focus: i64,
        bad_selects: usize,
    }
    let state = Arc::new(Mutex::new(YesNo {
        window: 10100,
        focus: 10,
        bad_selects: 0,
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let kodi = state.clone();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve(socket, move |method| {
            let mut dialog = kodi.lock().unwrap();
            match method {
                "GUI.GetProperties" => {
                    json!({"currentwindow": {"id": dialog.window}})
                }
                "XBMC.GetInfoBooleans" => json!({
                    "Control.HasFocus(11)": dialog.focus == 11,
                    "Control.HasFocus(10)": dialog.focus == 10,
                    "Control.HasFocus(12)": dialog.focus == 12,
                    "System.HasActiveModalDialog": dialog.window == 10100,
                }),
                "Input.Left" => {
                    // Grouplist order is Yes (11), No (10), then 12.
                    let order = [11_i64, 10, 12];
                    let at = order.iter().position(|id| *id == dialog.focus).unwrap_or(0);
                    if at > 0 {
                        dialog.focus = order[at - 1];
                    }
                    json!("OK")
                }
                "Input.Right" => {
                    let order = [11_i64, 10, 12];
                    let at = order.iter().position(|id| *id == dialog.focus).unwrap_or(0);
                    if at + 1 < order.len() {
                        dialog.focus = order[at + 1];
                    }
                    json!("OK")
                }
                "Input.Select" => {
                    if dialog.focus == 11 {
                        dialog.window = 10000;
                    } else {
                        dialog.bad_selects += 1;
                    }
                    json!("OK")
                }
                "Input.Back" => {
                    dialog.bad_selects += 1;
                    json!("OK")
                }
                other => panic!("unexpected method {other}"),
            }
        })
        .await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    prompts
        .answer(&PromptAnswer {
            id: "w10100-1".into(),
            action: PromptAction::Yes,
            text: None,
            index: None,
        })
        .await
        .unwrap();
    {
        let dialog = state.lock().unwrap();
        assert_eq!(dialog.window, 10000);
        assert_eq!(dialog.focus, 11);
        assert_eq!(dialog.bad_selects, 0);
    }
    drop(prompts);
    client.close().await;
}

#[tokio::test]
async fn a_confirm_closed_before_select_still_counts() {
    struct YesNo {
        window: i64,
        focus: i64,
        selects: usize,
    }
    let state = Arc::new(Mutex::new(YesNo {
        window: 10100,
        focus: 10,
        selects: 0,
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let kodi = state.clone();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve(socket, move |method| {
            let mut dialog = kodi.lock().unwrap();
            match method {
                "GUI.GetProperties" => json!({"currentwindow": {"id": dialog.window}}),
                "XBMC.GetInfoLabels" => json!({
                    "Control.GetLabel(1)": "Allow?",
                    "Control.GetLabel(9)": "Install this provider?",
                    "Control.GetLabel(12)": ""
                }),
                "XBMC.GetInfoBooleans" => json!({
                    "Control.HasFocus(11)": dialog.focus == 11,
                    "Control.HasFocus(10)": dialog.focus == 10,
                    "System.HasActiveModalDialog": dialog.window == 10100,
                    "System.HasVisibleModalDialog": false,
                }),
                "Input.Left" | "Input.Right" => {
                    // The viewer clicked the revealed dialog while focus was walking.
                    dialog.window = 10000;
                    json!("OK")
                }
                "Input.Select" | "Input.Back" => {
                    dialog.selects += 1;
                    json!("OK")
                }
                other => panic!("unexpected method {other}"),
            }
        })
        .await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    let id = prompts.read().await.unwrap().expect("yes no").id;
    assert_eq!(id, "w10100-1");
    prompts
        .answer(&PromptAnswer {
            id,
            action: PromptAction::Yes,
            text: None,
            index: None,
        })
        .await
        .unwrap();
    {
        let dialog = state.lock().unwrap();
        assert_eq!(dialog.window, 10000);
        assert_eq!(dialog.selects, 0);
    }
    state.lock().unwrap().window = 10100;
    let again = prompts.read().await.unwrap().expect("asked again").id;
    assert_eq!(again, "w10100-2");
    drop(prompts);
    client.close().await;
}

#[tokio::test]
async fn a_select_that_stays_open_is_not_answered() {
    let kodi = late_select(3);
    let port = serve_late_select(kodi.clone()).await;
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    let id = prompts.read().await.unwrap().expect("select").id;
    let missed = prompts.answer(&submit(&id, 1)).await;
    assert!(matches!(missed, Err(EngineError::BadRequest(_))));
    assert_eq!(kodi.lock().unwrap().selected, Some(1));
    assert_eq!(kodi.lock().unwrap().window, 12000);
    assert_eq!(prompts.read().await.unwrap().expect("still asking").id, id);
    drop(prompts);
    client.close().await;
}

#[tokio::test]
async fn a_confirm_that_ignores_no_is_not_answered() {
    struct YesNo {
        window: i64,
        focus: i64,
    }
    let state = Arc::new(Mutex::new(YesNo {
        window: 10100,
        focus: 10,
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let kodi = state.clone();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve(socket, move |method| {
            let dialog = kodi.lock().unwrap();
            match method {
                "GUI.GetProperties" => json!({"currentwindow": {"id": dialog.window}}),
                "XBMC.GetInfoLabels" => json!({
                    "Control.GetLabel(1)": "Replace?",
                    "Control.GetLabel(9)": "This file exists.",
                    "Control.GetLabel(12)": ""
                }),
                "XBMC.GetInfoBooleans" => json!({
                    "Control.HasFocus(11)": dialog.focus == 11,
                    "Control.HasFocus(10)": dialog.focus == 10,
                    "System.HasActiveModalDialog": true,
                    "System.HasVisibleModalDialog": false,
                }),
                "Input.Select" | "Input.Back" | "Input.Left" | "Input.Right" => json!("OK"),
                other => panic!("unexpected method {other}"),
            }
        })
        .await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    let id = prompts.read().await.unwrap().expect("yes no").id;
    let missed = prompts
        .answer(&PromptAnswer {
            id: id.clone(),
            action: PromptAction::No,
            text: None,
            index: None,
        })
        .await;
    assert!(matches!(missed, Err(EngineError::BadRequest(_))));
    assert_eq!(state.lock().unwrap().window, 10100);
    assert_eq!(prompts.read().await.unwrap().expect("still asking").id, id);
    drop(prompts);
    client.close().await;
}

async fn answer_yesno(socket: tokio::net::TcpStream) {
    serve(socket, |method| match method {
        "GUI.GetProperties" => json!({"currentwindow": {"id": 10100}}),
        "XBMC.GetInfoLabels" => json!({
            "Control.GetLabel(1)": "Replace?",
            "Control.GetLabel(9)": "This file exists."
        }),
        other => panic!("unexpected method {other}"),
    })
    .await;
}

/// Dialogs in these fakes are open, not closing: a check made only of
/// `Window.IsActive(...)` is answered true for every window asked about.
async fn serve(socket: tokio::net::TcpStream, reply: impl Fn(&str) -> Value) {
    serve_params(socket, move |method, params| {
        if method == "XBMC.GetInfoBooleans" {
            if let Some(keys) = window_checks(params) {
                return Value::Object(
                    keys.into_iter()
                        .map(|key| (key, Value::Bool(true)))
                        .collect(),
                );
            }
        }
        reply(method)
    })
    .await;
}

fn window_checks(params: &Value) -> Option<Vec<String>> {
    let keys: Vec<String> = params
        .get("booleans")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    (!keys.is_empty() && keys.iter().all(|key| key.starts_with("Window.IsActive("))).then_some(keys)
}

async fn serve_params(mut socket: tokio::net::TcpStream, reply: impl Fn(&str, &Value) -> Value) {
    let mut stream = crate::rpc::JsonObjectStream::new();
    let mut buf = [0u8; 8192];
    let mut raw = Vec::new();
    loop {
        let count = match socket.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        raw.extend_from_slice(&buf[..count]);
        let valid = match std::str::from_utf8(&raw) {
            Ok(text) => text.len(),
            Err(err) => err.valid_up_to(),
        };
        if valid == 0 {
            continue;
        }
        let text = String::from_utf8(raw[..valid].to_vec()).unwrap();
        raw.drain(..valid);
        for object in stream.push(&text) {
            let request: Value = serde_json::from_str(&object).unwrap();
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let method = request.get("method").and_then(Value::as_str).unwrap_or("");
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let result = reply(method, &params);
            let body = json!({"jsonrpc": "2.0", "id": id, "result": result});
            socket
                .write_all(&serde_json::to_vec(&body).unwrap())
                .await
                .unwrap();
            socket.flush().await.unwrap();
        }
    }
}

#[tokio::test]
async fn a_top_dialog_that_is_closing_reads_as_stuck_and_not_as_a_prompt() {
    // Kodi still counts a modal as active, but the yes/no on top is closing.
    let closing = Arc::new(Mutex::new(true));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let kodi = closing.clone();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve_params(socket, move |method, params| match method {
            "GUI.GetProperties" => json!({"currentwindow": {"id": 10100}}),
            "XBMC.GetInfoBooleans" => {
                if window_checks(params).is_some() {
                    json!({"Window.IsActive(10100)": !*kodi.lock().unwrap()})
                } else {
                    json!({
                        "System.HasVisibleModalDialog": true,
                        "System.HasActiveModalDialog": true
                    })
                }
            }
            "XBMC.GetInfoLabels" => json!({
                "Control.GetLabel(1)": "Replace?",
                "Control.GetLabel(9)": "This file exists."
            }),
            other => panic!("unexpected method {other}"),
        })
        .await;
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let prompts = Prompts::new(client.clone());
    assert!(prompts.modal_close_stuck().await);
    assert_eq!(prompts.read().await.unwrap(), None);
    *closing.lock().unwrap() = false;
    assert!(!prompts.modal_close_stuck().await);
    assert_eq!(
        prompts.read().await.unwrap().expect("open again").kind,
        PromptKind::Yesno
    );
    drop(prompts);
    client.close().await;
}
