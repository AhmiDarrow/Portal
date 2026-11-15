//! Which encoder makes the picture: the processor, or a graphics card.
//!
//! A graphics card encodes many times faster and leaves the processor
//! free. Whether one is there, and whether its driver takes Portal's own
//! settings, is found by encoding a second of black with them once. A card
//! that then fails on a real video is not used again until Portal restarts.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    Cpu,
    Nvidia,
    Intel,
    Amd,
}

impl Encoder {
    /// A short name for the dashboard and the log.
    pub fn name(self) -> &'static str {
        match self {
            Encoder::Cpu => "cpu",
            Encoder::Nvidia => "nvidia",
            Encoder::Intel => "intel",
            Encoder::Amd => "amd",
        }
    }

    pub fn is_hardware(self) -> bool {
        self != Encoder::Cpu
    }
}

/// The graphics cards looked for, in the order they are tried.
const CARDS: [Encoder; 3] = [Encoder::Nvidia, Encoder::Intel, Encoder::Amd];
const PROBE_LIMIT: Duration = Duration::from_secs(15);
/// The rate the trial runs at, so a card that refuses a ceiling is found out.
const PROBE_RATE: u64 = 8_000_000;

/// What the user asked for and what this PC turned out to have.
pub(super) struct Hardware {
    /// False keeps every encode on the processor.
    wanted: bool,
    /// None until the first look.
    found: Option<Encoder>,
    /// A card that failed on a real video.
    failed: bool,
}

impl Hardware {
    pub(super) fn new() -> Self {
        Self {
            wanted: true,
            found: None,
            failed: false,
        }
    }
}

/// The codec and its quality settings. `maxrate` caps the busy scenes.
///
/// Each aims at the same look as the processor's own settings: a steady
/// quality, with a ceiling over a two-second buffer.
pub(super) fn encode_args(encoder: Encoder, maxrate: Option<u64>) -> Vec<String> {
    let mut args: Vec<String> = match encoder {
        Encoder::Cpu => vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "21"],
        Encoder::Nvidia => vec![
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p5",
            "-tune",
            "hq",
            "-rc",
            "vbr",
            "-cq",
            "23",
            "-b:v",
            "0",
            "-profile:v",
            "high",
            // A forced keyframe must be one a player can start on.
            "-forced-idr",
            "1",
        ],
        Encoder::Intel => vec![
            "-c:v",
            "h264_qsv",
            "-preset",
            "medium",
            "-global_quality",
            "23",
            "-profile:v",
            "high",
            "-forced_idr",
            "1",
        ],
        Encoder::Amd => vec![
            "-c:v",
            "h264_amf",
            "-quality",
            "balanced",
            "-profile:v",
            "high",
            "-forced_idr",
            "1",
        ],
    }
    .into_iter()
    .map(String::from)
    .collect();
    if encoder == Encoder::Amd {
        // This encoder has no steady-quality mode with a ceiling.
        match maxrate {
            Some(rate) => args.extend([
                "-rc".into(),
                "vbr_peak".into(),
                "-b:v".into(),
                (rate * 7 / 10).to_string(),
            ]),
            None => args.extend(
                ["-rc", "cqp", "-qp_i", "21", "-qp_p", "23"]
                    .into_iter()
                    .map(String::from),
            ),
        }
    }
    if let Some(rate) = maxrate {
        args.extend([
            "-maxrate".into(),
            rate.to_string(),
            "-bufsize".into(),
            (rate * 2).to_string(),
        ]);
    }
    args
}

/// Input options that have the graphics card read the picture too, which
/// halves what is left for the processor. A picture the card cannot read is
/// read by the processor as before; ffmpeg decides that by itself.
pub(super) fn decode_args(encoder: Encoder) -> Vec<String> {
    if encoder.is_hardware() {
        vec!["-hwaccel".into(), "d3d11va".into()]
    } else {
        Vec::new()
    }
}

/// A second of black through the encoder, with the settings a real video gets.
pub(super) fn probe_args(encoder: Encoder) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=1280x720:r=24",
        "-frames:v",
        "24",
        "-vf",
        "format=yuv420p",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    args.extend(encode_args(encoder, Some(PROBE_RATE)));
    args.extend(
        ["-bf", "0", "-force_key_frames", "expr:gte(t,n_forced*1)", "-f", "null", "-"]
            .into_iter()
            .map(String::from),
    );
    args
}

/// The first graphics card that encodes, or the processor.
fn look(ffmpeg: &Path) -> Encoder {
    CARDS
        .into_iter()
        .find(|card| {
            capture(ffmpeg, &probe_args(*card), PROBE_LIMIT)
                .is_ok_and(|done| done.status.success())
        })
        .unwrap_or(Encoder::Cpu)
}

impl Player {
    fn lock_hardware(&self) -> std::sync::MutexGuard<'_, Hardware> {
        self.inner
            .hardware
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Whether a graphics card may encode. Streams already running keep
    /// the encoder they started with.
    pub fn set_hardware(&self, wanted: bool) {
        self.lock_hardware().wanted = wanted;
    }

    /// Whether a graphics card may encode, as last set.
    pub fn hardware_wanted(&self) -> bool {
        self.lock_hardware().wanted
    }

    /// The graphics card this PC has for encoding, looked for once.
    /// `Encoder::Cpu` when there is none that works.
    pub async fn hardware(&self) -> Encoder {
        if let Some(found) = self.lock_hardware().found {
            return found;
        }
        let ffmpeg = self.inner.ffmpeg.clone();
        let found = tokio::task::spawn_blocking(move || look(&ffmpeg))
            .await
            .unwrap_or(Encoder::Cpu);
        tracing::info!(encoder = found.name(), "Looked for a graphics card to encode with");
        self.lock_hardware().found = Some(found);
        found
    }

    /// The encoder the next stream uses.
    pub async fn encoder(&self) -> Encoder {
        {
            let hardware = self.lock_hardware();
            if !hardware.wanted || hardware.failed {
                return Encoder::Cpu;
            }
        }
        self.hardware().await
    }

    /// A card that could not encode a real video is left alone from here on.
    pub(super) fn hardware_failed(&self) {
        let mut hardware = self.lock_hardware();
        if !hardware.failed {
            tracing::warn!("The graphics card could not encode; using the processor from here on");
        }
        hardware.failed = true;
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/engine/src/playback/encoder.rs"]
mod tests;
