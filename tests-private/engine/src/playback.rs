use super::*;

#[test]
fn a_modal_dialog_blocks_opening_the_next_play() {
    assert!(modal_blocks_open(true, true));
    assert!(modal_blocks_open(true, false));
    assert!(modal_blocks_open(false, true));
    assert!(!modal_blocks_open(false, false));
}

#[test]
fn only_a_wrong_episode_of_the_same_season_is_refused() {
    let request = |extra: serde_json::Value| -> PlayRequest {
        let mut body = json!({"kind": "episode", "id": "show", "file": "plugin://plugin.video.example/?play=1", "title": "Pilot"});
        body.as_object_mut()
            .expect("an object")
            .extend(extra.as_object().expect("an object").clone());
        serde_json::from_value(body).expect("a play request")
    };
    let asked = request(json!({"season": 4, "episode": 1}));
    assert_eq!(requested_episode(&asked), Some((4, 1)));
    assert!(refuses_episode(4, 4));
    assert!(!refuses_episode(4, 1));
    assert_eq!(episode_code(4, 19), "S04E19");
    let no_season = request(json!({"episode": 1}));
    assert_eq!(requested_episode(&no_season), None);
}

#[test]
fn ready_playback_does_not_block_an_account_restart() {
    assert!(blocks_engine_restart(JobState::Working));
    assert!(blocks_engine_restart(JobState::Prompt));
    assert!(blocks_engine_restart(JobState::Browse));
    assert!(!blocks_engine_restart(JobState::Ready));
    assert!(!blocks_engine_restart(JobState::Failed));
}

#[test]
fn video_plan_copies_safe_h264_and_scales_the_rest() {
    assert_eq!(
        video_plan("h264", "yuv420p", 720, Quality::Auto),
        VideoPlan {
            copy: true,
            scale: None,
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("h264", "yuvj420p", 720, Quality::Auto),
        VideoPlan {
            copy: true,
            scale: None,
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("h264", "yuv420p", 2000, Quality::Auto),
        VideoPlan {
            copy: false,
            scale: Some(1080),
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("hevc", "yuv420p", 720, Quality::Auto),
        VideoPlan {
            copy: false,
            scale: None,
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("h264", "yuv420p", 1080, Quality::P720),
        VideoPlan {
            copy: false,
            scale: Some(720),
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("h264", "yuv420p", 480, Quality::Source),
        VideoPlan {
            copy: true,
            scale: None,
            maxrate: None,
        }
    );
    // Full range plays as it is, at Source as at the capped qualities.
    assert_eq!(
        video_plan("h264", "yuvj420p", 480, Quality::Source),
        VideoPlan {
            copy: true,
            scale: None,
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("h264", "yuv420p", 400, Quality::P480),
        VideoPlan {
            copy: true,
            scale: None,
            maxrate: None,
        }
    );
}

#[test]
fn four_k_copies_safe_h264_and_never_scales_down_to_1080() {
    assert_eq!(
        video_plan("h264", "yuv420p", 2160, Quality::P2160),
        VideoPlan {
            copy: true,
            scale: None,
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("hevc", "yuv420p10le", 2160, Quality::P2160),
        VideoPlan {
            copy: false,
            scale: None,
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("hevc", "yuv420p", 4320, Quality::P2160),
        VideoPlan {
            copy: false,
            scale: Some(2160),
            maxrate: None,
        }
    );
    assert_eq!(
        video_plan("hevc", "yuv420p10le", 2160, Quality::Auto),
        VideoPlan {
            copy: false,
            scale: Some(1080),
            maxrate: None,
        }
    );
}

#[test]
fn only_timeouts_are_tried_again() {
    let message = |text: &str| EngineError::Message(text.into());
    assert!(worth_retry(&message(SLOW_START)));
    assert!(worth_retry(&message("Timed out waiting for the add-on.")));
    assert!(worth_retry(&message(
        "Could not read the stream: timed out"
    )));
    assert!(worth_retry(&message(
        "Transcoding failed: Connection timed out"
    )));
    assert!(worth_retry(&EngineError::Rpc(
        crate::rpc::RpcFail::Timeout("Player.Open".into())
    )));
    assert!(worth_retry(&EngineError::Rpc(
        crate::rpc::RpcFail::ConnectTimeout
    )));
    // Waiting on the user, a cancel or a missing stream would only fail again.
    assert!(!worth_retry(&message(NO_CHOICE)));
    assert!(!worth_retry(&message(CANCELLED)));
    assert!(!worth_retry(&message(NO_PLAYABLE_STREAM)));
    assert!(!worth_retry(&message("Stopped")));
    assert!(!worth_retry(&EngineError::NotFound));
}

#[test]
fn a_heavy_picture_is_reencoded_under_a_ceiling() {
    let mut stream = coded("h264", 0);
    // A web download stays a copy; so does a file that says nothing.
    stream.bitrate = Some(6_000_000);
    assert!(stream_plan(&stream, Quality::Auto).copy);
    stream.bitrate = None;
    assert!(stream_plan(&stream, Quality::P1080).copy);
    // A Blu-ray rip averaging 14 Mbit/s is capped at 12.
    stream.bitrate = Some(14_000_000);
    let plan = stream_plan(&stream, Quality::P1080);
    assert!(!plan.copy);
    assert_eq!(plan.maxrate, Some(12_000_000));
    let args = video_args(&plan, "", false, false);
    assert_eq!(after(&args, "-maxrate"), Some("12000000"));
    assert_eq!(after(&args, "-bufsize"), Some("24000000"));
    // Source means the file as it is.
    assert_eq!(stream_plan(&stream, Quality::Source).maxrate, None);
    assert!(stream_plan(&stream, Quality::Source).copy);
    // A smaller picture gets a smaller ceiling, whatever made it re-encode.
    let mut tall = coded("hevc", 0);
    tall.height = 2160;
    assert_eq!(stream_plan(&tall, Quality::P720).maxrate, Some(6_000_000));
    let plain = VideoPlan {
        copy: false,
        scale: None,
        maxrate: None,
    };
    assert_eq!(
        after(&video_args(&plain, "", false, false), "-maxrate"),
        None
    );
}

#[test]
fn bitrates_come_from_the_stream_its_tags_or_the_file() {
    let payload = json!({
        "streams": [
            {"index": 0, "codec_type": "video", "codec_name": "h264", "tags": {"BPS-eng": "14123456"}},
            {"index": 1, "codec_type": "audio", "codec_name": "dts", "bit_rate": "1509000"},
            {"index": 2, "codec_type": "audio", "codec_name": "ac3", "tags": {"BPS": "640000"}}
        ],
        "format": {"duration": "2600.5", "bit_rate": "16500000"}
    });
    let probe = parse_probe(&payload);
    assert_eq!(probe.video[0].bitrate, Some(14_123_456));
    assert_eq!(probe.audio[0].bitrate, Some(1_509_000));
    assert_eq!(probe.audio[1].bitrate, Some(640_000));
    assert_eq!(probe.bitrate, Some(16_500_000));
    assert_eq!(video_bitrate(&probe, &probe.video[0]), Some(14_123_456));
    // Without its own figure, the picture is the file less its audio.
    let mut bare = probe.video[0].clone();
    bare.bitrate = None;
    assert_eq!(
        video_bitrate(&probe, &bare),
        Some(16_500_000 - 1_509_000 - 640_000)
    );
    let silent =
        parse_probe(&json!({"streams": [{"index": 0, "codec_type": "video"}], "format": {}}));
    assert_eq!(video_bitrate(&silent, &silent.video[0]), None);
}

fn coded(codec: &str, index: i64) -> StreamInfo {
    StreamInfo {
        index: Some(index),
        codec: codec.into(),
        pix_fmt: "yuv420p".into(),
        height: 1080,
        transfer: String::new(),
        interlaced: false,
        is_default: true,
        forced: false,
        lang: None,
        title: None,
        bitrate: None,
    }
}

fn h264_with(audio: &str) -> MediaSource {
    let audio = coded(audio, 1);
    MediaSource {
        input: "https://cdn.example/a.mkv".into(),
        headers: Vec::new(),
        video: Some(coded("h264", 0)),
        audios: vec![audio.clone()],
        audio: Some(audio),
        subs: Vec::new(),
        duration: Some(3000.0),
        quality: Quality::Auto,
        title: String::new(),
    }
}

fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == flag)?;
    args.get(at + 1).map(String::as_str)
}

#[test]
fn a_browser_ready_picture_is_copied_whatever_the_audio() {
    let source = h264_with("ac3");
    let plan = video_plan("h264", "yuv420p", 1080, Quality::Auto);
    assert!(plan.copy);
    let args = ffmpeg_args(&source, 0, Path::new("out"), Some(&plan), false).unwrap();
    assert_eq!(after(&args, "-c:v"), Some("copy"));
    assert_eq!(after(&args, "-c:a"), Some("aac"));
    assert!(!args.iter().any(|arg| arg == "-copyts"));
    assert!(!args.iter().any(|arg| arg == "-noaccurate_seek"));
}

#[test]
fn a_copied_skip_keeps_the_files_clock() {
    let source = h264_with("ac3");
    let plan = video_plan("h264", "yuv420p", 1080, Quality::Auto);
    let args = ffmpeg_args(&source, 600, Path::new("out"), Some(&plan), false).unwrap();
    let input = args.iter().position(|arg| arg == "-i").unwrap();
    let loose = args
        .iter()
        .position(|arg| arg == "-noaccurate_seek")
        .unwrap();
    let clock = args.iter().position(|arg| arg == "-copyts").unwrap();
    assert!(loose < input && input < clock);
    assert_eq!(after(&args, "-ss"), Some("600"));
    // A re-encoded picture starts on the asked second without them.
    let encoded = video_plan("hevc", "yuv420p10le", 2160, Quality::Auto);
    let args = ffmpeg_args(&source, 600, Path::new("out"), Some(&encoded), false).unwrap();
    assert!(!args
        .iter()
        .any(|arg| arg == "-copyts" || arg == "-noaccurate_seek"));
    assert_eq!(after(&args, "-ss"), Some("600"));
}

#[test]
fn ffmpeg_reads_a_burst_then_a_few_times_faster_than_playback() {
    let source = h264_with("ac3");
    let plan = video_plan("h264", "yuv420p", 1080, Quality::Auto);
    for seek in [0, 600] {
        let args = ffmpeg_args(&source, seek, Path::new("out"), Some(&plan), false).unwrap();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        let rate = args.iter().position(|arg| arg == "-readrate").unwrap();
        let burst = args
            .iter()
            .position(|arg| arg == "-readrate_initial_burst")
            .unwrap();
        assert!(rate < input && burst < input);
        assert_eq!(after(&args, "-readrate"), Some("4"));
        assert_eq!(after(&args, "-readrate_initial_burst"), Some("30"));
    }
}

fn audio_stream(index: i64, lang: &str, title: &str, default: bool) -> StreamInfo {
    let mut stream = coded("aac", index);
    stream.is_default = default;
    stream.lang = if lang.is_empty() {
        None
    } else {
        Some(lang.into())
    };
    stream.title = if title.is_empty() {
        None
    } else {
        Some(title.into())
    };
    stream
}

#[test]
fn a_preferred_language_beats_the_default_track() {
    let streams = vec![
        audio_stream(1, "jpn", "", true),
        audio_stream(2, "eng", "", false),
    ];
    assert_eq!(pick_audio(&streams, Some("en")).unwrap().index, Some(2));
    assert_eq!(pick_audio(&streams, Some("en-US")).unwrap().index, Some(2));
    assert_eq!(pick_audio(&streams, Some("eng")).unwrap().index, Some(2));
    assert_eq!(pick_audio(&streams, None).unwrap().index, Some(1));
    assert_eq!(pick_audio(&streams, Some("de")).unwrap().index, Some(1));
    let (tracks, index) = audio_listing(&streams, pick_audio(&streams, Some("en")));
    assert_eq!(index, Some(2));
    assert_eq!(tracks[0].label, "Japanese");
    assert_eq!(tracks[1].label, "English");
    assert_eq!(tracks[1].lang.as_deref(), Some("en"));
}

#[test]
fn commentary_loses_to_the_main_track_of_the_same_language() {
    let streams = vec![
        audio_stream(1, "eng", "Director commentary", true),
        audio_stream(2, "en", "", false),
        audio_stream(3, "eng", "Descriptive audio", false),
    ];
    assert_eq!(pick_audio(&streams, Some("en")).unwrap().index, Some(2));
    let (tracks, _) = audio_listing(&streams, None);
    assert_eq!(tracks[0].label, "English (commentary)");
    assert_eq!(tracks[1].label, "English");
    assert_eq!(tracks[2].label, "English (descriptive)");
    let twins = vec![
        audio_stream(1, "eng", "", false),
        audio_stream(2, "eng", "", false),
    ];
    let (tracks, _) = audio_listing(&twins, None);
    assert_eq!(tracks[0].label, "English");
    assert_eq!(tracks[1].label, "English 2");
    // With no language asked for, the file's default still wins.
    let commentary_default = vec![
        audio_stream(1, "eng", "Commentary", true),
        audio_stream(2, "jpn", "", false),
    ];
    assert_eq!(
        pick_audio(&commentary_default, None).unwrap().index,
        Some(1)
    );
}

#[test]
fn the_chosen_audio_stream_is_the_one_mapped() {
    let mut source = h264_with("aac");
    let chosen = audio_stream(4, "eng", "", false);
    source.audio = Some(chosen);
    let plan = video_plan("h264", "yuv420p", 1080, Quality::Auto);
    let args = ffmpeg_args(&source, 0, Path::new("out"), Some(&plan), false).unwrap();
    assert!(args
        .windows(2)
        .any(|pair| pair[0] == "-map" && pair[1] == "0:4"));
}

#[test]
fn flac_is_reencoded_because_browsers_cannot_play_it_from_ts() {
    let args = audio_args(Some(&coded("flac", 1)));
    assert_eq!(after(&args, "-c:a"), Some("aac"));
    let args = audio_args(Some(&coded("AAC", 1)));
    assert_eq!(after(&args, "-c:a"), Some("copy"));
    let args = audio_args(Some(&coded("mp3", 1)));
    assert_eq!(after(&args, "-c:a"), Some("copy"));
}

#[test]
fn segments_and_keyframes_share_one_length() {
    let source = h264_with("aac");
    let encoded = video_plan("hevc", "yuv420p10le", 2160, Quality::Auto);
    let args = ffmpeg_args(&source, 0, Path::new("out"), Some(&encoded), false).unwrap();
    assert_eq!(after(&args, "-hls_time"), Some("2"));
    assert_eq!(
        after(&args, "-force_key_frames"),
        Some("expr:gte(t,n_forced*2)")
    );
}

#[test]
fn an_odd_sized_picture_is_cropped_to_even_before_x264() {
    let plan = video_plan("hevc", "yuv420p", 803, Quality::Auto);
    assert_eq!(plan.scale, None);
    let args = video_args(&plan, "", false, false);
    assert_eq!(
        after(&args, "-vf"),
        Some("crop=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p")
    );
    // A scaled picture is already even.
    let scaled = video_plan("hevc", "yuv420p", 2160, Quality::Auto);
    let args = video_args(&scaled, "", false, false);
    assert_eq!(after(&args, "-vf"), Some("scale=-2:1080,format=yuv420p"));
}

#[test]
fn hdr_tagged_only_by_its_transfer_still_tone_maps() {
    let plan = video_plan("hevc", "yuv420p10le", 2160, Quality::Auto);
    let args = video_args(&plan, "smpte2084", false, true);
    assert!(after(&args, "-vf").unwrap().starts_with(
            "scale=-2:1080,setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc,zscale=t=linear"
        ));
    let hlg = video_args(&plan, "ARIB-STD-B67", false, true);
    assert!(after(&hlg, "-vf")
        .unwrap()
        .contains(":color_trc=arib-std-b67:"));
    // Without zscale the picture is only converted, never retagged.
    let plain = video_args(&plan, "smpte2084", false, false);
    assert!(!after(&plain, "-vf").unwrap().contains("setparams"));
}

#[test]
fn an_interlaced_picture_is_deinterlaced_before_it_is_resized() {
    let probe = parse_probe(&json!({"streams": [
        {"index": 0, "codec_type": "video", "codec_name": "mpeg2video", "field_order": "tt"},
        {"index": 1, "codec_type": "video", "codec_name": "h264", "field_order": "BB"},
        {"index": 2, "codec_type": "video", "codec_name": "h264", "field_order": "progressive"},
        {"index": 3, "codec_type": "video", "codec_name": "h264"},
    ]}));
    let woven: Vec<bool> = probe.video.iter().map(|stream| stream.interlaced).collect();
    assert_eq!(woven, [true, true, false, false]);
    let plan = video_plan("mpeg2video", "yuv420p", 1080, Quality::Auto);
    let args = video_args(&plan, "", true, false);
    assert_eq!(
            after(&args, "-vf"),
            Some("bwdif=mode=send_frame:parity=auto:deint=interlaced,crop=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p")
        );
    let scaled = video_plan("mpeg2video", "yuv420p", 2160, Quality::Auto);
    let args = video_args(&scaled, "smpte2084", true, true);
    let chain = after(&args, "-vf").unwrap();
    assert!(chain.starts_with(
        "bwdif=mode=send_frame:parity=auto:deint=interlaced,scale=-2:1080,setparams="
    ));
    // A progressive picture gets no deinterlacer.
    let args = video_args(&plan, "", false, false);
    assert!(!after(&args, "-vf").unwrap().contains("bwdif"));
}

#[test]
fn an_interlaced_picture_is_re_encoded_unless_the_source_is_asked_for() {
    let mut source = h264_with("aac");
    let video = source.video.as_mut().unwrap();
    video.interlaced = true;
    assert!(!stream_plan(video, Quality::Auto).copy);
    assert!(stream_plan(video, Quality::Source).copy);
    video.interlaced = false;
    assert!(stream_plan(video, Quality::Auto).copy);
}

#[test]
fn a_copied_interlaced_picture_is_left_alone() {
    let mut source = h264_with("aac");
    if let Some(video) = source.video.as_mut() {
        video.interlaced = true;
    }
    let plan = video_plan("h264", "yuv420p", 1080, Quality::Auto);
    assert!(plan.copy);
    let args = ffmpeg_args(&source, 0, Path::new("out"), Some(&plan), false).unwrap();
    assert_eq!(after(&args, "-c:v"), Some("copy"));
    assert!(!args.iter().any(|arg| arg == "-vf"));
}

#[test]
fn a_copied_start_is_the_keyframe_before_the_asked_time() {
    // Read from ffmpeg's own output: a skip to 20 s on a file with a
    // keyframe at 18.018 s starts its first segment at 19.418 s.
    assert_eq!(
        keyframe_start(19.418, 20).map(|at| (at * 1000.0).round()),
        Some(18018.0)
    );
    assert_eq!(
        keyframe_start(21.42, 25).map(|at| (at * 1000.0).round()),
        Some(20020.0)
    );
    // Never past the asked time, and never far from it.
    assert_eq!(keyframe_start(21.6, 20), Some(20.0));
    assert_eq!(keyframe_start(30.0, 20), None);
    assert_eq!(keyframe_start(1.4, 900), None);
    assert_eq!(keyframe_start(f64::NAN, 20), None);
}

#[test]
fn player_open_starts_at_the_beginning() {
    let params = player_open_params("plugin://example/?action=play");
    assert_eq!(params["item"]["file"], "plugin://example/?action=play");
    assert_eq!(params["options"]["resume"], false);
}

#[test]
fn blank_yesno_has_no_label_text() {
    assert!(labels_are_blank(&[String::new(), " ".into(), "\n".into()]));
    assert!(!labels_are_blank(&[
        "Resume from 1:00".into(),
        String::new()
    ]));
    assert!(yesno_copy_is_blank("", "", ""));
    assert!(yesno_copy_is_blank("  ", "", ""));
    assert!(!yesno_copy_is_blank("Resume", "", ""));
    assert!(!yesno_copy_is_blank("", "Continue from 41:54?", ""));
}

#[test]
fn dynpath_is_the_stream_when_the_item_file_stays_virtual() {
    assert_eq!(
        direct_item_path(
            "plugin://video/play",
            "https://cdn.example/a.mp4",
            "plugin://video/play"
        )
        .as_deref(),
        Some("https://cdn.example/a.mp4")
    );
    assert_eq!(
        direct_item_path("https://cdn.example/a.mp4", "", "").as_deref(),
        Some("https://cdn.example/a.mp4")
    );
    assert_eq!(
        direct_item_path(
            "plugin://video/play",
            "plugin://video/play",
            "plugin://video/play"
        ),
        None
    );
    assert_eq!(
        direct_item_path("", "", r"D:\media\a.mkv").as_deref(),
        Some(r"D:\media\a.mkv")
    );
}

#[test]
fn questions_hold_the_resolve_and_progress_does_not() {
    assert!(user_is_answering(10100));
    assert!(user_is_answering(12000));
    assert!(user_is_answering(10103));
    assert!(!user_is_answering(10101));
    assert!(!user_is_answering(10138));
    assert!(!user_is_answering(10000));
    // An add-on's own source list holds the resolve too.
    assert!(user_is_answering(13000));
    assert!(user_is_answering(13099));
    assert!(!user_is_answering(13100));
}

#[test]
fn input_args_cover_http_headers_and_skip_local_and_hls() {
    let headers = [("User-Agent", "Portal"), ("Referer", "http://example.test")];
    let args = input_args("https://cdn.example/a.mp4", &headers);
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-user_agent", "Portal"]));
    let header = args
        .iter()
        .find(|arg| arg.contains("Referer:"))
        .expect("header");
    assert_eq!(header, "Referer: http://example.test\r\n");
    assert!(args.iter().any(|arg| arg == "-reconnect"));
    assert_eq!(after(&args, "-rw_timeout"), Some("60000000"));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-reconnect_on_network_error", "1"]));
    assert!(input_args(r"C:\video.mp4", &headers).is_empty());
    let hls = input_args("https://cdn.example/a.m3u8?x=1", &[]);
    assert_eq!(after(&hls, "-rw_timeout"), Some("60000000"));
    assert!(!hls.iter().any(|arg| arg == "-reconnect"));
    assert!(!hls.iter().any(|arg| arg == "-reconnect_on_network_error"));
    let injected = input_args("http://a/b", &[("X", "1\r\nInjected")]);
    let value = injected
        .iter()
        .find(|arg| arg.starts_with("X:"))
        .expect("x");
    assert_eq!(value, "X: 1Injected\r\n");
    assert!(!value.contains('\n') || value.ends_with("\r\n"));
    assert!(!value[..value.len() - 2].contains('\n'));
}

#[test]
fn playback_waits_for_a_runway_before_it_starts() {
    let thin = PlaylistDepth {
        seconds: 2.0,
        segments: 1,
        ended: false,
    };
    assert!(!runway_ready(&thin, Some(Duration::from_secs(1))));
    assert!(runway_ready(&thin, Some(START_GRACE)));
    let runway = PlaylistDepth {
        seconds: 12.0,
        segments: 6,
        ended: false,
    };
    assert!(runway_ready(&runway, Some(Duration::from_secs(1))));
    let done = PlaylistDepth {
        seconds: 2.0,
        segments: 1,
        ended: true,
    };
    assert!(runway_ready(&done, None));
    let empty = PlaylistDepth {
        seconds: 0.0,
        segments: 0,
        ended: true,
    };
    assert!(!runway_ready(&empty, Some(START_GRACE)));
    let text = "#EXTM3U\n#EXTINF:2.0,\nseg00000.ts\n#EXTINF:10.5,\nseg00001.ts\n";
    let depth = depth_of(text);
    assert_eq!(depth.segments, 2);
    assert!((depth.seconds - 12.5).abs() < 0.01);
    assert!(!depth.ended);
}

#[test]
fn unsafe_inputs_are_rejected() {
    assert!(assert_safe_input("-hide_banner").is_err());
    assert!(assert_safe_input("http://a/b\n-f").is_err());
    assert!(assert_safe_input("http://a/b").is_ok());
}

#[test]
fn moving_progress_resets_only_when_it_advances() {
    assert!(progress_advanced(None, 0.0));
    assert!(!progress_advanced(Some(0.62), 0.62));
    assert!(progress_advanced(Some(0.62), 0.80));
    assert!(!progress_advanced(Some(0.80), 0.62));
}

#[test]
fn dismiss_leaves_the_extended_progress_dialog() {
    assert!(should_dismiss_window(10101));
    assert!(should_dismiss_window(10138));
    assert!(!should_dismiss_window(10160));
    assert!(!should_dismiss_window(10100));
    assert!(should_dismiss_window(13001));
}

#[test]
fn seek_ignores_a_minute_or_less() {
    assert_eq!(seek_seconds(None), 0);
    assert_eq!(seek_seconds(Some(60.0)), 0);
    assert_eq!(seek_seconds(Some(60.1)), 60);
    assert_eq!(seek_seconds(Some(90.9)), 90);
    assert_eq!(seek_seconds(Some(f64::NAN)), 0);
}

#[test]
fn skip_to_keeps_a_short_time() {
    assert_eq!(skip_to(0.0, None).unwrap(), 0);
    assert_eq!(skip_to(30.2, None).unwrap(), 30);
    assert_eq!(skip_to(60.0, Some(100.0)).unwrap(), 60);
    assert_eq!(skip_to(500.0, Some(90.9)).unwrap(), 80);
    assert!(skip_to(f64::NAN, None).is_err());
    assert!(skip_to(-1.0, Some(100.0)).is_err());
    assert_eq!(seek_seconds(Some(30.0)), 0);
}

#[test]
fn a_resume_past_the_end_starts_before_it() {
    assert_eq!(before_end(5400, Some(3000.5)), 2990);
    assert_eq!(before_end(120, Some(3000.0)), 120);
    assert_eq!(before_end(120, Some(4.0)), 0);
    assert_eq!(before_end(120, None), 120);
    assert_eq!(before_end(120, Some(f64::NAN)), 120);
}

#[test]
fn a_failed_move_can_be_tried_again() {
    let source = MediaSource {
        input: "https://cdn.example/a.mkv".into(),
        headers: Vec::new(),
        video: None,
        audio: None,
        audios: Vec::new(),
        subs: Vec::new(),
        duration: Some(100.0),
        quality: Quality::Auto,
        title: String::new(),
    };
    let mut slot = test_slot("job4", JobState::Failed);
    assert!(!can_seek(&slot));
    slot.source = Some(source);
    assert!(can_seek(&slot));
    slot.job.state = JobState::Ready;
    assert!(can_seek(&slot));
    slot.job.state = JobState::Working;
    assert!(!can_seek(&slot));
}

#[tokio::test]
async fn a_quality_change_reuses_the_read_stream() {
    let root = test_root("quality");
    let player = Player::new(root.join("no-ffmpeg.exe"), "ffprobe", &root);
    let mut slot = test_slot("job6", JobState::Working);
    slot.source = Some(MediaSource {
        input: "https://cdn.example/a.mkv".into(),
        headers: Vec::new(),
        video: None,
        audio: None,
        audios: Vec::new(),
        subs: Vec::new(),
        duration: Some(100.0),
        quality: Quality::Auto,
        title: String::new(),
    });
    player.lock_jobs().insert("job6".into(), slot);
    assert!(player
        .set_quality("job6", Quality::P720, 30.0)
        .await
        .is_err());
    assert_eq!(player.quality("job6"), Some(Quality::Auto));
    player.lock_jobs().get_mut("job6").unwrap().job.state = JobState::Ready;
    // No Kodi client is involved; only ffmpeg runs again, and here it is missing.
    assert!(player
        .set_quality("job6", Quality::P720, 30.0)
        .await
        .is_err());
    assert_eq!(player.quality("job6"), Some(Quality::P720));
    let job = player.job("job6").unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert!(player
        .set_quality("missing", Quality::P720, 30.0)
        .await
        .is_err());
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn an_audio_change_keeps_the_read_stream_and_the_chosen_track() {
    let root = test_root("audio");
    let player = Player::new(root.join("no-ffmpeg.exe"), "ffprobe", &root);
    let japanese = audio_stream(1, "jpn", "", true);
    let english = audio_stream(2, "eng", "", false);
    let mut slot = test_slot("job7", JobState::Ready);
    slot.source = Some(MediaSource {
        input: "https://cdn.example/a.mkv".into(),
        headers: Vec::new(),
        video: None,
        audio: Some(japanese.clone()),
        audios: vec![japanese, english],
        subs: Vec::new(),
        duration: Some(100.0),
        quality: Quality::Auto,
        title: String::new(),
    });
    let cut = slot.cut;
    player.lock_jobs().insert("job7".into(), slot);
    let same = player.set_audio("job7", 1, 40.0).await.unwrap();
    assert_eq!(same.state, JobState::Ready);
    assert_eq!(player.lock_jobs().get("job7").unwrap().cut, cut);
    assert!(matches!(
        player.set_audio("job7", 9, 40.0).await,
        Err(EngineError::BadRequest(_))
    ));
    assert_eq!(
        player
            .lock_jobs()
            .get("job7")
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .audio
            .as_ref()
            .unwrap()
            .index,
        Some(1)
    );
    // ffmpeg is missing, so the restart fails, but the chosen track stays.
    assert!(player.set_audio("job7", 2, 40.0).await.is_err());
    let source = player
        .lock_jobs()
        .get("job7")
        .unwrap()
        .source
        .clone()
        .unwrap();
    assert_eq!(source.audio.as_ref().unwrap().index, Some(2));
    assert_eq!(source.input, "https://cdn.example/a.mkv");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn session_names_stay_inside_one_folder() {
    let player = Player::new("ffmpeg", "ffprobe", PathBuf::from("hls"));
    assert!(player.session_file("abc", "index.m3u8").is_ok());
    assert!(player.session_file("abc", "seg00001.ts").is_ok());
    assert!(player.session_file("abc", "sub00002.vtt").is_ok());
    assert!(player.session_file("abc", "cast.mp4").is_ok());
    assert!(matches!(
        player.session_file("abc", "cast.vod.m3u8"),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        player.session_file("abc", "cast.span"),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        player.session_file("abc", "cast.mp4.part"),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        player.session_file("abc", "index.m3u8.exe"),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        player.session_file("../abc", "index.m3u8"),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        player.session_file("abc", ".."),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        player.session_file("abc", "seg123.ts"),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        player.session_file("abc", "seg0001.ts"),
        Err(EngineError::NotFound)
    ));
}

#[test]
fn split_kodi_url_keeps_the_first_bar() {
    let (url, headers) =
        split_kodi_url("http://cdn/a.mp4|User-Agent=Portal&Referer=http%3A%2F%2Fex");
    assert_eq!(url, "http://cdn/a.mp4");
    assert_eq!(headers[0], ("User-Agent".into(), "Portal".into()));
    assert_eq!(headers[1].1, "http://ex");
    let (plain, none) = split_kodi_url("http://cdn/a.mp4");
    assert_eq!(plain, "http://cdn/a.mp4");
    assert!(none.is_empty());
}

#[test]
fn direct_inputs_keep_their_login_and_reject_virtual_urls() {
    let (url, _) = to_input("http://user:letmein@cdn.example/a.mp4|Referer=http://ex").unwrap();
    assert_eq!(url, "http://user:letmein@cdn.example/a.mp4");
    assert!(to_input("smb://server/share/a.mkv").is_err());
    let (stacked, _) = to_input("stack://C:\\a.mkv , C:\\b.mkv").unwrap();
    assert_eq!(stacked, r"C:\a.mkv");
}

#[test]
fn a_login_never_becomes_a_header_that_follows_redirects() {
    let (url, headers) = to_input("http://user:pa%3Ass+1@cdn.example/f.mkv").unwrap();
    assert_eq!(url, "http://user:pa%3Ass+1@cdn.example/f.mkv");
    assert!(headers.is_empty());
    // An add-on's own header is still passed on.
    let (_, kept) =
        to_input("http://user:pw@cdn.example/f.mkv|Authorization=Bearer%20abc").unwrap();
    assert_eq!(
        kept,
        vec![("Authorization".to_string(), "Bearer abc".to_string())]
    );
    // Error text never shows the login.
    assert!(!failure_line("http://user:pw@cdn.example/f.mkv: I/O error").contains("pw@"));
}

#[test]
fn ffmpeg_that_fails_before_the_end_marker_stopped_early() {
    use std::os::windows::process::ExitStatusExt;
    let failed = ExitStatus::from_raw(1);
    let clean = ExitStatus::from_raw(0);
    let open = "#EXTM3U\nindex0.ts\n";
    let done = "#EXTM3U\nindex0.ts\n#EXT-X-ENDLIST\n";
    assert!(stopped_early(failed, Some(open)));
    assert!(stopped_early(failed, None));
    assert!(!stopped_early(failed, Some(done)));
    assert!(!stopped_early(clean, Some(open)));
    assert_eq!(stopped_message(""), "Transcoding stopped.");
    let detail = failure_line("https://user:pw@cdn.example/f.mkv?t=1: I/O error");
    assert_eq!(
        stopped_message(&detail),
        "Transcoding stopped: https://cdn.example: I/O error"
    );
}

#[test]
fn a_ready_job_fails_when_ffmpeg_dies_mid_play() {
    use std::os::windows::process::CommandExt;
    let root = test_root("died");
    fs::write(root.join("index.m3u8"), "#EXTM3U\nindex0.ts\n").unwrap();
    let mut child = Command::new("cmd")
        .args(["/C", "exit 3"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let _ = child.wait();
    let mut slot = test_slot("job", JobState::Ready);
    slot.ffmpeg = Some(Arc::new(Mutex::new(child)));
    slot.dir = Some(root.clone());
    slot.stderr = Some(Arc::new(Mutex::new(
        "Error in the pull function for http://cdn.example/f.mkv?tok=1.\n".into(),
    )));
    note_ffmpeg_exit(&mut slot);
    assert_eq!(slot.job.state, JobState::Failed);
    assert_eq!(
        slot.job.error.as_deref(),
        Some("Transcoding stopped: Error in the pull function for http://cdn.example.")
    );
    remove_dir(&root);
}

#[tokio::test]
async fn a_slow_start_is_not_called_a_transcoding_failure() {
    use std::os::windows::process::CommandExt;
    let root = test_root("slow");
    let playlist = root.join("index.m3u8");
    let spawn = |script: &str| {
        Arc::new(Mutex::new(
            Command::new("cmd")
                .args(["/C", script])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .unwrap(),
        ))
    };
    let reading = spawn("ping -n 6 127.0.0.1 >nul");
    let slow =
        wait_for_segment_within(&reading, &playlist, &|| false, Duration::from_millis(300)).await;
    end_process(&reading);
    assert_eq!(slow.unwrap_err().to_string(), SLOW_START);
    let failed = spawn("exit 3");
    let early =
        wait_for_segment_within(&failed, &playlist, &|| false, Duration::from_secs(20)).await;
    end_process(&failed);
    assert_eq!(
        early.unwrap_err().to_string(),
        "Transcoding failed to start"
    );
    fs::write(&playlist, "#EXTM3U\nseg00000.ts\n").unwrap();
    let ready = spawn("exit 0");
    assert!(
        wait_for_segment_within(&ready, &playlist, &|| false, Duration::from_secs(20))
            .await
            .is_ok()
    );
    end_process(&ready);
    remove_dir(&root);
}

#[test]
fn text_subtitles_after_many_bitmap_ones_are_still_read() {
    let mut streams: Vec<Value> = (0..9)
        .map(|index| {
            json!({
                "index": index + 2,
                "codec_type": "subtitle",
                "codec_name": "hdmv_pgs_subtitle",
            })
        })
        .collect();
    streams.push(json!({
        "index": 11,
        "codec_type": "subtitle",
        "codec_name": "subrip",
        "disposition": {"forced": 1},
        "tags": {"language": "eng"},
    }));
    streams.push(json!({"index": 12, "codec_type": "subtitle", "codec_name": "ass"}));
    let probe = parse_probe(&json!({ "streams": streams }));
    let wanted: Vec<(u32, &StreamInfo)> = subtitle_streams(&probe.subs).collect();
    assert_eq!(wanted.len(), 2);
    assert_eq!(wanted[0].0, 10);
    assert!(wanted[0].1.forced);
    assert_eq!(wanted[1].0, 11);
    assert!(!wanted[1].1.forced);
    let many: Vec<Value> = (0..12)
        .map(|index| json!({"index": index, "codec_type": "subtitle", "codec_name": "subrip"}))
        .collect();
    let probe = parse_probe(&json!({ "streams": many }));
    assert_eq!(subtitle_streams(&probe.subs).count(), 8);
}

#[test]
fn one_ffmpeg_run_writes_each_wanted_subtitle_to_its_own_file() {
    let probe = parse_probe(&json!({"streams": [
        {"index": 3, "codec_type": "subtitle", "codec_name": "subrip"},
        {"index": 4, "codec_type": "subtitle", "codec_name": "hdmv_pgs_subtitle"},
        {"index": 5, "codec_type": "subtitle", "codec_name": "ass", "tags": {"title": "Signs"}},
    ]}));
    let wanted: Vec<(u32, &StreamInfo)> = subtitle_streams(&probe.subs).collect();
    let scratch = test_root("extract");
    let args = subtitle_args("https://cdn.example/a.mkv", &[], &wanted, &scratch);
    assert_eq!(args.iter().filter(|arg| *arg == "-i").count(), 1);
    let maps: Vec<&str> = args
        .windows(2)
        .filter(|pair| pair[0] == "-map")
        .map(|pair| pair[1].as_str())
        .collect();
    assert_eq!(maps, ["0:3", "0:5"]);
    let first = scratch.join("sub00001.vtt").to_string_lossy().into_owned();
    let third = scratch.join("sub00003.vtt").to_string_lossy().into_owned();
    assert!(args.contains(&first));
    assert!(args.contains(&third));
    fs::write(&first, "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHi\n").unwrap();
    let texts = collect_subtitles(&wanted, &scratch);
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0].id, 1);
    assert_eq!(texts[0].label, "Subtitles");
    fs::write(&third, "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nSign\n").unwrap();
    let texts = collect_subtitles(&wanted, &scratch);
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[1].label, "Signs");
    remove_dir(&scratch);
}

#[test]
fn shift_vtt_adds_converts_srt_and_clamps() {
    let srt = "1\n00:00:01,000 --> 00:00:02,500\nHello\n";
    let added = shift_vtt(srt, 1.0);
    assert!(added.starts_with("WEBVTT"));
    assert!(added.contains("00:00:02.000 --> 00:00:03.500"));
    assert!(added.contains("Hello"));
    assert!(!added.contains(','));
    let clamped = shift_vtt("WEBVTT\n\n00:00:01.000 --> 00:00:04.000\nHi\n", -2.0);
    assert!(clamped.contains("00:00:00.000 --> 00:00:02.000"));
    assert!(clamped.contains("Hi"));
}

#[test]
fn release_names_come_from_video_file_names_only() {
    assert_eq!(
        release_name("https://cdn.example/d/ABC123/Show.S01E01.720p.WEB-DL-GRP.mkv?token=x")
            .as_deref(),
        Some("Show.S01E01.720p.WEB-DL-GRP")
    );
    assert_eq!(
        release_name("https://cdn.example/files/Some%20Movie%20(1994)%201080p.mp4").as_deref(),
        Some("Some Movie (1994) 1080p")
    );
    assert_eq!(
        release_name(r"D:\Videos\Movie.2010.BluRay.x264-GRP.avi").as_deref(),
        Some("Movie.2010.BluRay.x264-GRP")
    );
    assert_eq!(release_name("https://cdn.example/play/12345"), None);
    assert_eq!(release_name("https://cdn.example/stream/index.m3u8"), None);
    assert_eq!(release_name("https://cdn.example/"), None);
    assert_eq!(release_name("https://cdn.example/a.mkv"), None);
}

#[test]
fn stderr_drops_userinfo_and_authorization() {
    let line = failure_line(
        "open https://user:letmein@cdn.example/a.mp4\nAuthorization: SECRETTOKEN\nboom",
    );
    assert_eq!(line, "boom");
    let redacted = redact("Authorization: SECRETTOKEN\nhttps://user:letmein@cdn.example/a");
    assert!(!redacted.contains("SECRETTOKEN"));
    assert!(!redacted.contains("letmein"));
    let bearer = redact("Authorization: Bearer SECRETTOKEN\r\nReferer: x");
    assert_eq!(bearer, "Authorization: \r\nReferer: x");
    assert!(redacted.contains("cdn.example"));
}

#[test]
fn stream_links_in_errors_keep_only_scheme_and_host() {
    let line = failure_line(
            "https://cdn.example/d/SECRETID/Show.S01E01.mkv?token=SECRET: Server returned 404 Not Found",
        );
    assert_eq!(
        line,
        "https://cdn.example: Server returned 404 Not Found"
    );
    assert_eq!(
        redact("open 'http://user:pw@cdn.example:8080/a/b.mp4' failed, retry http://cdn.example."),
        "open 'http://cdn.example:8080' failed, retry http://cdn.example."
    );
}

fn test_slot(id: &str, state: JobState) -> JobSlot {
    JobSlot {
        job: PlayJob {
            id: id.into(),
            state,
            error: None,
            session: None,
            playlist: None,
            subtitles: Vec::new(),
            subtitles_pending: false,
            next: None,
            browse_path: None,
            title: None,
            position: None,
            duration: None,
            release: None,
            audio: Vec::new(),
            audio_index: None,
            prompt: None,
            prompt_known: false,
        },
        ffmpeg: None,
        session: None,
        dir: None,
        stderr: None,
        source: None,
        cut: 1,
        subtitle_text: None,
        extracting: false,
        cast: None,
        parked: false,
        stream_touch: None,
        last_touch: Instant::now(),
        seen: Instant::now(),
        fetched: None,
        rested: false,
    }
}

fn test_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("portal-playback-{name}-{}", fresh_id()));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn idle_jobs_expire_on_their_own_clock() {
    let minute = Duration::from_secs(60);
    assert!(!idle_expired(JobState::Working, minute * 9));
    assert!(idle_expired(JobState::Working, minute * 11));
    assert!(idle_expired(JobState::Browse, minute * 11));
    assert!(idle_expired(JobState::Failed, minute * 11));
    assert!(!idle_expired(JobState::Ready, minute * 119));
    assert!(idle_expired(JobState::Ready, minute * 121));
}

#[tokio::test]
async fn the_sweep_stops_abandoned_jobs_and_removes_their_folder() {
    let root = test_root("sweep");
    let player = Player::new("ffmpeg", "ffprobe", &root);
    let dir = root.join("sess1");
    fs::create_dir_all(&dir).unwrap();
    let mut left = test_slot("left", JobState::Working);
    left.session = Some("sess1".into());
    left.dir = Some(dir.clone());
    player.lock_jobs().insert("left".into(), left);
    player.lock_sessions().insert("sess1".into(), "left".into());
    player
        .lock_jobs()
        .insert("watching".into(), test_slot("watching", JobState::Ready));
    player
        .sweep(Instant::now() + Duration::from_secs(11 * 60))
        .await;
    assert!(player.job("left").is_none());
    assert!(player.lock_sessions().is_empty());
    assert!(!dir.exists());
    assert!(player.job("watching").is_some());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn asking_for_a_job_or_its_files_keeps_it_alive() {
    let player = Player::new("ffmpeg", "ffprobe", PathBuf::from("hls"));
    let old = Instant::now() - Duration::from_secs(5);
    let mut slot = test_slot("job1", JobState::Ready);
    slot.session = Some("sess1".into());
    slot.last_touch = old;
    player.lock_jobs().insert("job1".into(), slot);
    player.lock_sessions().insert("sess1".into(), "job1".into());
    player.session_file("sess1", "seg00001.ts").unwrap();
    assert!(player.lock_jobs()["job1"].last_touch > old);
    player.lock_jobs().get_mut("job1").unwrap().last_touch = old;
    player.session_file("sess1", "index.m3u8").unwrap();
    assert_eq!(player.lock_jobs()["job1"].last_touch, old);
    player.session_file("sess1", "sub00001.vtt").unwrap();
    assert!(player.lock_jobs()["job1"].last_touch > old);
    player.lock_jobs().get_mut("job1").unwrap().last_touch = old;
    // A playing job is kept by its segments, not by a page polling it.
    player.job("sess1").unwrap();
    assert_eq!(player.lock_jobs()["job1"].last_touch, old);
    player.lock_jobs().get_mut("job1").unwrap().job.state = JobState::Working;
    player.job("sess1").unwrap();
    assert!(player.lock_jobs()["job1"].last_touch > old);
}

#[tokio::test]
async fn kept_subtitles_fill_the_playing_session_shifted_to_its_start() {
    let root = test_root("subs");
    let player = Player::new("ffmpeg", "ffprobe", &root);
    let dir = root.join("sess2");
    fs::create_dir_all(&dir).unwrap();
    let mut slot = test_slot("job2", JobState::Ready);
    slot.session = Some("sess2".into());
    slot.dir = Some(dir.clone());
    slot.job.position = Some(60.0);
    slot.job.subtitles_pending = true;
    player.lock_jobs().insert("job2".into(), slot);
    assert!(player.claim_extraction("job2"));
    assert!(!player.claim_extraction("job2"));
    player.fill_subtitles("job2").await;
    assert!(player.job("job2").unwrap().subtitles_pending);
    {
        let mut jobs = player.lock_jobs();
        let slot = jobs.get_mut("job2").unwrap();
        slot.extracting = false;
        slot.subtitle_text = Some(Arc::new(vec![SubtitleText {
            id: 3,
            label: "English".into(),
            lang: Some("eng".into()),
            forced: false,
            text: "WEBVTT\n\n00:01:05.000 --> 00:01:07.000\nHello\n".into(),
        }]));
    }
    assert!(!player.claim_extraction("job2"));
    player.fill_subtitles("job2").await;
    let job = player.job("job2").unwrap();
    assert!(!job.subtitles_pending);
    assert_eq!(job.subtitles.len(), 1);
    assert_eq!(job.subtitles[0].url, "/media/sess2/sub00003.vtt");
    let written = fs::read_to_string(dir.join("sub00003.vtt")).unwrap();
    assert!(written.contains("00:00:05.000 --> 00:00:07.000"));
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_background_read_never_revives_a_stopped_job() {
    let player = Player::new("ffmpeg", "ffprobe", PathBuf::from("hls"));
    let source = MediaSource {
        input: "https://cdn.example/a.mkv".into(),
        headers: Vec::new(),
        video: None,
        audio: None,
        audios: Vec::new(),
        subs: Vec::new(),
        duration: None,
        quality: Quality::Auto,
        title: String::new(),
    };
    player
        .clone()
        .extract_in_background("gone".into(), source.clone())
        .await;
    assert!(player.lock_jobs().is_empty());
    let mut slot = test_slot("job3", JobState::Working);
    slot.extracting = true;
    slot.job.subtitles_pending = true;
    player.lock_jobs().insert("job3".into(), slot);
    player
        .clone()
        .extract_in_background("job3".into(), source)
        .await;
    let jobs = player.lock_jobs();
    let slot = &jobs["job3"];
    assert!(!slot.extracting);
    assert!(slot
        .subtitle_text
        .as_ref()
        .is_some_and(|texts| texts.is_empty()));
    // A skip in progress lists them once its session is ready.
    assert!(slot.job.subtitles_pending);
}

#[tokio::test(start_paused = true)]
async fn a_replaced_session_folder_outlives_the_switch_then_goes() {
    let root = std::env::temp_dir().join(format!("portal-retire-{}", fresh_id()));
    let old = root.join("old");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("index.m3u8"), "#EXTM3U").unwrap();
    retire_dir(old.clone(), RETIRE_GRACE);
    tokio::time::sleep(RETIRE_GRACE - Duration::from_secs(1)).await;
    // A casting TV still fetches the old playlist during the grace.
    assert!(old.join("index.m3u8").exists());
    tokio::time::sleep(Duration::from_secs(2)).await;
    for _ in 0..200 {
        if !old.exists() {
            break;
        }
        tokio::time::advance(Duration::from_millis(10)).await;
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!old.exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_network_share_is_never_opened() {
    for share in [
        r"\\host\share\a.mkv",
        "//host/share/a.mkv",
        r"\/host/a.mkv",
        r"  \\host\a.mkv",
    ] {
        assert!(network_share(share), "{share}");
        assert!(to_input(share).is_err(), "{share}");
    }
    assert!(!network_share(r"C:\videos\a.mkv"));
    assert!(!network_share("https://example.com/a.mkv"));
}

#[test]
fn a_refused_certificate_is_found_above_the_last_line() {
    let schannel = b"[tls @ 0000] Peer certificate failed verification\r\nhttps://a.example/v.mp4: I/O error\r\n";
    assert!(certificate_refused(schannel));
    assert!(certificate_refused(
        b"[tls @ 0] error:0A000086:SSL routines::certificate verify failed\n"
    ));
    assert!(!certificate_refused(
        b"https://a.example/v.mp4: Server returned 404 Not Found\n"
    ));
    assert!(!certificate_refused(b""));
}

/// A local test file made by ffmpeg, or None where ffmpeg is not on the path.
fn made_video(root: &Path, picture: u32, sound: u32) -> Option<PathBuf> {
    let path = root.join(format!("v{picture}-a{sound}.mp4"));
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=s=320x180:r=24:d={picture}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=duration={sound}"))
        .args(["-c:v", "libx264", "-c:a", "aac"])
        .arg(&path)
        .status()
        .ok()?;
    status.success().then_some(path)
}

fn cast_player(root: &Path, input: &Path, duration: f64) -> Player {
    let player = Player::new("ffmpeg", "ffprobe", root);
    // The processor, so the test does not depend on the machine's card.
    player.hardware_failed();
    let mut slot = test_slot("cast", JobState::Ready);
    slot.source = Some(MediaSource {
        input: input.display().to_string(),
        headers: Vec::new(),
        video: Some(coded("h264", 0)),
        audios: vec![coded("aac", 1)],
        audio: Some(coded("aac", 1)),
        subs: Vec::new(),
        duration: Some(duration),
        quality: Quality::Auto,
        title: String::new(),
    });
    player.lock_jobs().insert("cast".into(), slot);
    player
}

/// The encoder runs that made pieces in a cast folder, by their first piece.
fn cast_runs(dir: &Path) -> std::collections::BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            name.ends_with(".m4s")
                .then(|| name.split('-').next().unwrap().to_string())
        })
        .collect()
}

#[tokio::test]
async fn sound_past_the_picture_is_cast_in_one_run() {
    let root = test_root("cast-tail");
    let Some(input) = made_video(&root, 12, 30) else {
        return;
    };
    let player = cast_player(&root, &input, 30.0);
    let session = player.cast_stream("cast").unwrap();
    for index in 0..10 {
        let piece = player
            .cast_file(&session, &format!("c{index:05}.m4s"))
            .await;
        assert!(piece.is_ok_and(|bytes| !bytes.is_empty()), "piece {index}");
    }
    let runs = cast_runs(&root.join(&session));
    assert_eq!(runs.into_iter().collect::<Vec<_>>(), ["r0"]);
    player.cast_rest("cast");
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_lone_far_piece_leaves_the_running_encoder_alone() {
    let root = test_root("cast-side");
    let Some(input) = made_video(&root, 180, 180) else {
        return;
    };
    let player = cast_player(&root, &input, 180.0);
    let session = player.cast_stream("cast").unwrap();
    let dir = root.join(&session);
    // Resuming at piece 10, the TV also reads the first piece once.
    assert!(player.cast_file(&session, "c00010.m4s").await.is_ok());
    assert!(player.cast_file(&session, "c00000.m4s").await.is_ok());
    assert!(player.cast_file(&session, "c00011.m4s").await.is_ok());
    assert!(player.cast_file(&session, "c00012.m4s").await.is_ok());
    let runs = cast_runs(&dir);
    assert!(runs.contains("r10") && runs.contains("r0"), "{runs:?}");
    assert!(!runs.contains("r11") && !runs.contains("r12"), "{runs:?}");
    // Read on from the far piece, and that run is the one kept.
    assert!(player.cast_file(&session, "c00055.m4s").await.is_ok());
    assert!(player.cast_file(&session, "c00056.m4s").await.is_ok());
    assert!(player.cast_file(&session, "c00057.m4s").await.is_ok());
    assert!(!cast_runs(&dir).contains("r56"));
    player.cast_rest("cast");
    let _ = fs::remove_dir_all(&root);
}

fn waiting_child() -> Arc<Mutex<Child>> {
    use std::os::windows::process::CommandExt;
    Arc::new(Mutex::new(
        Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >nul"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap(),
    ))
}

fn playlist_of(segments: u32) -> String {
    let mut text = String::from("#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n");
    for index in 0..segments {
        text.push_str(&format!("#EXTINF:2.0,\nseg{index:05}.ts\n"));
    }
    text
}

#[test]
fn a_quiet_page_is_abandoned_after_twenty_minutes_but_a_cast_is_not() {
    let minute = Duration::from_secs(60);
    assert!(!abandoned(JobState::Ready, false, minute * 19));
    assert!(abandoned(JobState::Ready, false, minute * 21));
    assert!(!abandoned(JobState::Ready, true, minute * 21));
    // Jobs still being made keep their own ten-minute clock.
    assert!(!abandoned(JobState::Working, false, minute * 21));
}

#[test]
fn only_an_encode_well_ahead_of_the_browser_rests() {
    assert_eq!(segment_index("seg00042.ts"), Some(42));
    assert_eq!(segment_index("index.m3u8"), None);
    assert_eq!(segment_index("sub00001.vtt"), None);
    assert!(!far_ahead(REST_LEAD - 1, None));
    assert!(far_ahead(REST_LEAD, None));
    assert!(!far_ahead(10 + REST_LEAD, Some(10)));
    assert!(far_ahead(11 + REST_LEAD, Some(10)));
}

#[test]
fn a_disk_message_says_how_much_room_is_wanted() {
    assert!(low_disk_message(START_FREE).contains("less than 3 GB free"));
    assert!(free_bytes(&std::env::temp_dir()).is_some_and(|free| free > 0));
}

#[tokio::test]
async fn a_paused_or_gone_viewer_rests_the_encode_and_a_segment_wakes_it() {
    let root = test_root("rest");
    let player = Player::new("ffmpeg", "ffprobe", &root);
    let dir = root.join("sess1");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("index.m3u8"), playlist_of(40)).unwrap();
    let child = waiting_child();
    let mut slot = test_slot("job1", JobState::Ready);
    slot.session = Some("sess1".into());
    slot.dir = Some(dir.clone());
    slot.ffmpeg = Some(child.clone());
    slot.fetched = Some(5);
    player.lock_jobs().insert("job1".into(), slot);
    player.lock_sessions().insert("sess1".into(), "job1".into());
    assert!(encoding(&player.lock_jobs()["job1"]));
    player.sweep(Instant::now() + Duration::from_secs(2 * 60)).await;
    assert!(!player.lock_jobs()["job1"].parked);
    player.sweep(Instant::now() + Duration::from_secs(4 * 60)).await;
    {
        let jobs = player.lock_jobs();
        assert!(jobs["job1"].parked && jobs["job1"].rested);
        assert!(!encoding(&jobs["job1"]));
    }
    // A paused player still reloads the playlist; that keeps the job, not the encode.
    player.session_file("sess1", "index.m3u8").unwrap();
    assert!(player.lock_jobs()["job1"].parked);
    // Play fetches the next listed segment, and the encode runs on.
    player.session_file("sess1", "seg00006.ts").unwrap();
    {
        let jobs = player.lock_jobs();
        assert!(!jobs["job1"].parked && !jobs["job1"].rested);
        assert_eq!(jobs["job1"].fetched, Some(6));
    }
    assert!(!child_exited(&child));
    end_process(&child);
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn an_encode_close_to_the_viewer_keeps_running() {
    let root = test_root("close");
    let player = Player::new("ffmpeg", "ffprobe", &root);
    let dir = root.join("sess1");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("index.m3u8"), playlist_of(20)).unwrap();
    let child = waiting_child();
    let mut slot = test_slot("job1", JobState::Ready);
    slot.dir = Some(dir.clone());
    slot.ffmpeg = Some(child.clone());
    slot.fetched = Some(10);
    player.lock_jobs().insert("job1".into(), slot);
    player.sweep(Instant::now() + Duration::from_secs(4 * 60)).await;
    assert!(!player.lock_jobs()["job1"].parked);
    end_process(&child);
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_page_that_stops_asking_loses_its_stream_but_a_paused_one_keeps_it() {
    let root = test_root("gone");
    let player = Player::new("ffmpeg", "ffprobe", &root);
    let gone_dir = root.join("sess1");
    fs::create_dir_all(&gone_dir).unwrap();
    let old = Instant::now() - Duration::from_secs(21 * 60);
    let mut gone = test_slot("gone", JobState::Ready);
    gone.session = Some("sess1".into());
    gone.dir = Some(gone_dir.clone());
    gone.last_touch = old;
    gone.seen = old;
    player.lock_jobs().insert("gone".into(), gone);
    player.lock_sessions().insert("sess1".into(), "gone".into());
    let mut paused = test_slot("paused", JobState::Ready);
    paused.last_touch = old;
    paused.seen = old;
    player.lock_jobs().insert("paused".into(), paused);
    // The paused page's heartbeat.
    player.job("paused").unwrap();
    player.sweep(Instant::now()).await;
    assert!(player.job("gone").is_none());
    assert!(!gone_dir.exists());
    assert!(player.job("paused").is_some());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_full_drive_ends_the_encode_with_a_reason() {
    let child = waiting_child();
    let mut slot = test_slot("job1", JobState::Ready);
    slot.ffmpeg = Some(child.clone());
    let taken = out_of_room(&mut slot).unwrap();
    assert_eq!(slot.job.state, JobState::Failed);
    assert!(slot.job.error.as_deref().unwrap().contains("less than 1 GB free"));
    assert!(slot.ffmpeg.is_none());
    end_process(&taken);
}

#[test]
fn new_plays_are_refused_past_the_encode_cap() {
    let player = Player::new("ffmpeg", "ffprobe", PathBuf::from("hls"));
    for index in 0..MAX_ENCODES {
        let id = format!("job{index}");
        player.lock_jobs().insert(id.clone(), test_slot(&id, JobState::Working));
    }
    // A finished stream costs nothing and does not count.
    player.lock_jobs().insert("done".into(), test_slot("done", JobState::Ready));
    let request: PlayRequest = serde_json::from_value(
        json!({"kind": "file", "id": "x", "file": "C:/a.mp4", "title": "A"}),
    )
    .unwrap();
    let client = KodiClient::new("127.0.0.1", 1);
    let err = player.start(request, client, None).unwrap_err();
    assert!(err.to_string().contains("already making 4 videos"));
}
