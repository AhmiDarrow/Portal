use super::*;

fn after<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == name)?;
    args.get(at + 1).map(String::as_str)
}

#[test]
fn the_processor_keeps_its_settings() {
    let args = encode_args(Encoder::Cpu, Some(12_000_000));
    assert_eq!(after(&args, "-c:v"), Some("libx264"));
    assert_eq!(after(&args, "-crf"), Some("21"));
    assert_eq!(after(&args, "-maxrate"), Some("12000000"));
    assert_eq!(after(&args, "-bufsize"), Some("24000000"));
    assert!(after(&encode_args(Encoder::Cpu, None), "-maxrate").is_none());
}

#[test]
fn each_card_gets_its_own_codec_and_a_ceiling() {
    for (card, codec) in [
        (Encoder::Nvidia, "h264_nvenc"),
        (Encoder::Intel, "h264_qsv"),
        (Encoder::Amd, "h264_amf"),
    ] {
        let args = encode_args(card, Some(8_000_000));
        assert_eq!(after(&args, "-c:v"), Some(codec));
        assert_eq!(after(&args, "-maxrate"), Some("8000000"));
        assert_eq!(after(&args, "-bufsize"), Some("16000000"));
        assert!(card.is_hardware());
        assert!(after(&encode_args(card, None), "-maxrate").is_none());
    }
    assert!(!Encoder::Cpu.is_hardware());
    // A forced keyframe is one a player can start on.
    assert_eq!(
        after(&encode_args(Encoder::Nvidia, None), "-forced-idr"),
        Some("1")
    );
    assert_eq!(
        after(&encode_args(Encoder::Amd, Some(10_000_000)), "-b:v"),
        Some("7000000")
    );
    assert_eq!(after(&encode_args(Encoder::Amd, None), "-rc"), Some("cqp"));
    assert_eq!(
        after(&encode_args(Encoder::Intel, Some(10_000_000)), "-b:v"),
        Some("7000000")
    );
    assert!(after(&encode_args(Encoder::Intel, None), "-b:v").is_none());
}

#[test]
fn the_card_reads_the_picture_only_when_it_also_encodes() {
    assert!(decode_args(Encoder::Cpu).is_empty());
    assert_eq!(decode_args(Encoder::Nvidia), vec!["-hwaccel", "d3d11va"]);
    assert_eq!(decode_args(Encoder::Amd), vec!["-hwaccel", "d3d11va"]);
}

#[test]
fn an_encoder_is_known_by_its_short_name() {
    for encoder in [Encoder::Cpu, Encoder::Nvidia, Encoder::Intel, Encoder::Amd] {
        assert_eq!(Encoder::named(encoder.name()), Some(encoder));
    }
    assert_eq!(Encoder::named(" NVIDIA "), Some(Encoder::Nvidia));
    assert_eq!(Encoder::named("auto"), None);
    assert_eq!(Encoder::named(""), None);
}

#[tokio::test]
async fn a_named_encoder_is_used_without_looking() {
    let player = Player::new("ffmpeg-that-is-not-there", "ffprobe", std::env::temp_dir());
    player.assume_hardware(Encoder::Amd);
    assert_eq!(player.hardware().await, Encoder::Amd);
    assert_eq!(player.encoder().await, Encoder::Amd);
}

#[test]
fn the_trial_uses_the_real_settings() {
    let args = probe_args(Encoder::Nvidia);
    assert_eq!(after(&args, "-c:v"), Some("h264_nvenc"));
    assert_eq!(after(&args, "-maxrate"), Some("8000000"));
    assert_eq!(after(&args, "-f"), Some("lavfi"));
    assert_eq!(args.last().map(String::as_str), Some("-"));
}

#[tokio::test]
async fn the_processor_is_used_when_asked_or_after_a_failure() {
    let player = Player::new("ffmpeg-that-is-not-there", "ffprobe", std::env::temp_dir());
    assert!(player.hardware_wanted());
    // No ffmpeg, so no card is found.
    assert_eq!(player.hardware().await, Encoder::Cpu);
    assert_eq!(player.encoder().await, Encoder::Cpu);
    player.lock_hardware().found = Some(Encoder::Nvidia);
    assert_eq!(player.encoder().await, Encoder::Nvidia);
    player.set_hardware(false);
    assert_eq!(player.encoder().await, Encoder::Cpu);
    player.set_hardware(true);
    assert_eq!(player.encoder().await, Encoder::Nvidia);
    player.hardware_failed();
    assert_eq!(player.encoder().await, Encoder::Cpu);
    // What the PC has is still reported.
    assert_eq!(player.hardware().await, Encoder::Nvidia);
}
