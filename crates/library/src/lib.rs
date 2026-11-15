//! Portal's watch state: continue watching, watchlist, bookmarks, and history.
//!
//! Title ids are add-on paths.

mod rules;
mod store;

pub use rules::{
    continuing, finished, fraction, next_episode, parse_key, progress_key, resumable, title_key,
    ContinuePick, EpisodeNum, LastPlayed, ParsedKey, ProgressMark, SeasonCount, RESUME_AFTER,
    WATCHED_AT,
};
pub use store::{PlayRoute, Store, StoreError, TitleFacts};
