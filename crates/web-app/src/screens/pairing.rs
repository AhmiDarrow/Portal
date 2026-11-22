//! Pairing this device with the PC (pair.ts; plan P2): shown by the shell instead of the
//! pages after a 401 with `{"pair": true}`. It asks Portal for a number, shows it as
//! "123 456", and asks every 2 s whether the PC accepted it; a new number comes when it
//! expires, and a failed start tries again after 30 s. Once paired, the page loads fresh.

use std::time::Duration;

use portal_api::PairState;
use remedy_ui::a11y::{Live, Role, Semantics};
use remedy_ui::layout::{Align, Length};
use remedy_ui::reactive::{Cancel, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet, TextAlign};
use remedy_ui::style::LetterSpacing;
use remedy_ui::view::{RichText, Style, TextStylePatch};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api;
use crate::theme::Look;
use crate::Ctx;

/// How often the page asks whether the PC accepted the number (pair.ts `POLL_MS`).
pub const POLL: Duration = Duration::from_millis(2000);
/// How long it waits before asking for a number again after Portal did not answer.
pub const RETRY: Duration = Duration::from_secs(30);

/// "123 456" from "123456"; anything else as it is.
pub fn spaced(code: &str) -> String {
    if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_owned()
    }
}

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    ctx.window.set_title("Pair this device · Portal");
    let code = Signal::new("······".to_owned());
    let note = Signal::new(String::new());
    start(ctx, code, note);
    let wording = RichText::new()
        .text("On the PC running Portal, open ")
        .styled("Server", TextStylePatch::bold())
        .text(" and type this number under ")
        .styled("Devices", TextStylePatch::bold())
        .text(".");
    let width = ctx.width;
    ui.el("p_pair")
        .column()
        .align(Align::Center)
        .fill_width()
        .semantics(Semantics::new(Role::Main).label("Pair this device"))
        .text_align(TextAlign::Center)
        .layout(|l| {
            l.max_width = Length::Px(544.0);
            l.align_self = Some(Align::Center);
            l.padding.start = 16.0;
            l.padding.end = 16.0;
        })
        .bind_top(ctx)
        .child(w::heading(ui, 1, "Pair this device"))
        .child(
            ui.el("p_pair_code")
                .text(move || RichText::plain(code.get()))
                .semantics(move || Semantics::new(Role::Label).label(code.get()).live(Live::Polite))
                // clamp(2.6rem, 10vw, 5rem)
                .style(move || Style::new().text_size((width.get() * 0.1).clamp(41.6, 80.0)))
                .layout(|l| {
                    l.margin.top = 19.2;
                    l.margin.bottom = 19.2;
                }),
        )
        .child(ui.el("p_pair_text").text(wording))
        .child(
            w::text(ui, move || note.get())
                .variant("muted")
                .semantics(move || Semantics::new(Role::Status).label(note.get()).live(Live::Polite))
                .layout(|l| l.margin.top = 16.0),
        )
}

trait TopMargin {
    fn bind_top(self, ctx: &Ctx) -> El;
}

impl TopMargin for El {
    /// `margin: 12vh auto 0`.
    fn bind_top(self, ctx: &Ctx) -> El {
        let (e, h) = (self.clone(), ctx.height);
        self.bind(move || {
            let top = h.get() * 0.12;
            let _ = e.clone().layout(move |l| l.margin.top = top);
        })
    }
}

/// pair.ts `run`: a number, then polls until it is accepted or expires.
fn start(ctx: &Ctx, code: Signal<String>, note: Signal<String>) {
    let c = ctx.clone();
    ctx.api.run(api::pair_start(), &Cancel::new(), move |out| match out {
        Ok(started) => {
            code.set(spaced(&started.code));
            note.set(format!("The number works for {} minutes.", (started.expires_in as f64 / 60.0).round()));
            poll(&c, started.ticket, code, note);
        }
        Err(e) => {
            note.set(if e.status.is_some() { e.detail } else { "Portal did not answer.".into() });
            let c2 = c.clone();
            c.activity.ui().after(RETRY, move || start(&c2, code, note));
        }
    });
}

fn poll(ctx: &Ctx, ticket: String, code: Signal<String>, note: Signal<String>) {
    let c = ctx.clone();
    ctx.activity.ui().after(POLL, move || {
        let c2 = c.clone();
        c.api.run(api::pair_status(&ticket), &Cancel::new(), move |out| match out.map(|s| s.state) {
            Ok(PairState::Paired) => c2.app.reload(),
            Ok(PairState::Expired) => start(&c2, code, note),
            // Waiting, or no answer this time: ask again.
            _ => poll(&c2, ticket, code, note),
        });
    });
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    // `.pair-code`: clamp(2.6rem, 10vw, 5rem) in gold, spaced, tabular figures.
    s.rule("p_pair_code", None, States::NONE, StylePatch::default().foreground(k.gold).letter_spacing(LetterSpacing::Em(0.12)));
    s.rule("p_pair_text", None, States::NONE, StylePatch::default().foreground(k.text));
}

#[cfg(test)]
mod tests {
    #[test]
    fn shows_the_number_in_two_halves() {
        assert_eq!(super::spaced("123456"), "123 456");
        assert_eq!(super::spaced("12345"), "12345");
    }
}
