//! Kodi stays a JSON-RPC engine. Portal's dashboard never opens Kodi's windows.
//!
//! `status` never starts Kodi. `ensure` attaches when port 9090 is already open
//! and only spawns a process when nothing is listening. Hide, quit, and restart
//! apply only to a Kodi this process started.

mod addons;
mod catalog;
mod dialog_catalog;
mod error;
mod event;
pub mod kodi_info;
mod episode_name;
mod playback;
mod process;
mod prompts;
mod roots;
mod rpc;
mod settings;
mod sources;

pub use addons::{Addons, InstallOutcome, RemoveOutcome};
pub use catalog::{
    browse, classify, directory_params, execute_plugin, plain_label, play_refusal, plugin_link,
    run_plugin_builtin, search, settings_addon, settings_name_hint, video_addons,
};
pub use error::{public_error, EngineError};
pub use playback::{
    picks_source, release_name, shift_vtt, source_cap, Encoder, Player, CAST_INIT, CAST_PLAYLIST,
};
pub use process::{public_ip, public_web_host, ProcessConfig, Supervisor};
pub use prompts::Prompts;
pub use rpc::{KodiClient, Note, RpcFail};
pub use settings::Settings;
pub use sources::{pack_identity, pack_named, pick_source, row_height};
