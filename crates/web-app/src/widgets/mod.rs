//! Pieces several screens share, one file each: Portal's buttons and links (`controls`),
//! art and the letter tile (`art`), cards (`card`, main.ts `cardHtml`), shelves (`shelf`),
//! episode titles and clocks (`titles`), the activity panel and notice (`activity`) and
//! page frames (`page`).

pub mod activity;
pub mod art;
pub mod browse_card;
pub mod card;
pub mod controls;
pub mod episode_row;
pub mod glyphs;
pub mod hero;
pub mod page;
pub mod play;
pub mod saved;
pub mod season_tabs;
pub mod shelf;
pub mod shows;
pub mod titles;

pub use controls::{button, link};

use remedy_ui::style::StyleSheet;

use crate::theme::Look;

/// The shared widgets' rules in Portal's sheet ([`crate::theme::sheet`]).
pub fn rules(s: &mut StyleSheet, k: &Look) {
    controls::rules(s, k);
    art::rules(s, k);
    card::rules(s, k);
    shelf::rules(s, k);
    activity::rules(s, k);
    saved::rules(s, k);
    crate::screens::info::rules(s, k);
    hero::rules(s, k);
    season_tabs::rules(s, k);
    episode_row::rules(s, k);
    browse_card::rules(s, k);
    crate::screens::show::rules(s, k);
    crate::screens::title::rules(s, k);
    crate::screens::prompts::rules(s, k);
    crate::screens::player::rules(s, k);
}
