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

    /// The encoder a short name stands for, as `name` writes it.
    pub fn named(name: &str) -> Option<Self> {
        [Encoder::Cpu, Encoder::Nvidia, Encoder::Intel, Encoder::Amd]
            .into_iter()
            .find(|encoder| encoder.name() == name.trim().to_ascii_lowercase())
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
    #[cfg(target_os = "linux")]
    if matches!(encoder, Encoder::Intel | Encoder::Amd) {
        return vaapi_encode_args(maxrate);
    }
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
    if encoder == Encoder::Intel {
        // A quality target with a ceiling needs a rate to aim at as well.
        if let Some(rate) = maxrate {
            args.extend(["-b:v".into(), (rate * 7 / 10).to_string()]);
        }
    }
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
#[cfg(windows)]
pub(super) fn decode_args(encoder: Encoder) -> Vec<String> {
    if encoder.is_hardware() {
        vec!["-hwaccel".into(), "d3d11va".into()]
    } else {
        Vec::new()
    }
}

/// On Linux an NVIDIA card reads through CUDA; Intel and AMD cards through
/// VA-API, on the render node that also takes the upload for the encoder.
#[cfg(target_os = "linux")]
pub(super) fn decode_args(encoder: Encoder) -> Vec<String> {
    match encoder {
        Encoder::Cpu => Vec::new(),
        Encoder::Nvidia => vec!["-hwaccel".into(), "cuda".into()],
        Encoder::Intel | Encoder::Amd => {
            let mut args: Vec<String> = vec!["-hwaccel".into(), "vaapi".into()];
            if let Some(node) = render_node() {
                let node = node.to_string_lossy().into_owned();
                args.extend(["-hwaccel_device".into(), node.clone()]);
                args.extend(["-vaapi_device".into(), node]);
            }
            args
        }
    }
}

/// The last filters of the picture: the pixel format the encoder takes, and,
/// for VA-API, the upload to the card.
pub(super) fn picture_tail(encoder: Encoder) -> &'static str {
    #[cfg(target_os = "linux")]
    if matches!(encoder, Encoder::Intel | Encoder::Amd) {
        return VAAPI_TAIL;
    }
    let _ = encoder;
    "format=yuv420p"
}

/// Adds `filter` to a `-vf` chain, ahead of a VA-API upload, which must stay last.
pub(super) fn add_filter(chain: &mut String, filter: &str) {
    #[cfg(target_os = "linux")]
    if let Some(head) = chain.strip_suffix(VAAPI_TAIL) {
        *chain = format!("{head}{filter},{VAAPI_TAIL}");
        return;
    }
    chain.push(',');
    chain.push_str(filter);
}

#[cfg(target_os = "linux")]
const VAAPI_TAIL: &str = "format=nv12,hwupload";

/// The first DRM render node, as VA-API's own tools pick it.
#[cfg(target_os = "linux")]
fn render_node() -> Option<PathBuf> {
    let mut nodes: Vec<PathBuf> = fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().next()
}

/// Whether the render node's card is `card`'s make, so an AMD card is never
/// called Intel. NVIDIA is tried through NVENC, not here.
#[cfg(target_os = "linux")]
fn render_node_is(card: Encoder) -> bool {
    let Some(name) = render_node().and_then(|node| Some(node.file_name()?.to_owned())) else {
        return false;
    };
    let vendor = Path::new("/sys/class/drm").join(name).join("device").join("vendor");
    let vendor = fs::read_to_string(vendor).unwrap_or_default();
    match card {
        Encoder::Intel => vendor.trim() == "0x8086",
        Encoder::Amd => vendor.trim() == "0x1002",
        _ => false,
    }
}

/// VA-API's H.264: a steady quality, or, under a ceiling, a variable rate
/// aimed below it (VA-API has no steady-quality mode with a ceiling).
#[cfg(target_os = "linux")]
fn vaapi_encode_args(maxrate: Option<u64>) -> Vec<String> {
    let mut args: Vec<String> = ["-c:v", "h264_vaapi", "-profile:v", "high"]
        .into_iter()
        .map(String::from)
        .collect();
    match maxrate {
        Some(rate) => args.extend([
            "-rc_mode".into(),
            "VBR".into(),
            "-b:v".into(),
            (rate * 7 / 10).to_string(),
            "-maxrate".into(),
            rate.to_string(),
            "-bufsize".into(),
            (rate * 2).to_string(),
        ]),
        None => args.extend(["-rc_mode", "CQP", "-qp", "23"].into_iter().map(String::from)),
    }
    args
}

/// A second of black through the encoder, with the settings a real video gets.
pub(super) fn probe_args(encoder: Encoder) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error"]
        .into_iter()
        .map(String::from)
        .collect();
    // VA-API needs its device named before the input; d3d11va decode does not apply here.
    #[cfg(target_os = "linux")]
    args.extend(decode_args(encoder));
    args.extend(
        [
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=1280x720:r=24",
            "-frames:v",
            "24",
            "-vf",
            picture_tail(encoder),
        ]
        .into_iter()
        .map(String::from),
    );
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
            // Intel and AMD share VA-API on Linux; the card's make decides the name.
            #[cfg(target_os = "linux")]
            if *card != Encoder::Nvidia && !render_node_is(*card) {
                return false;
            }
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

    /// Names the encoder this PC has, in place of looking for one. For a PC
    /// where the look picks wrongly, and for trying a card's settings.
    pub fn assume_hardware(&self, encoder: Encoder) {
        self.lock_hardware().found = Some(encoder);
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
